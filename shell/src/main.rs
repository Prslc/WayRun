mod app;
mod cli;
mod config;
mod session;
mod wayland;

use std::env;
use std::error::Error;
use std::path::Path;
use std::process;
use std::time::Instant;

use calloop::EventLoop;
use calloop::channel::Event as ChannelEvent;
use calloop_wayland_source::WaylandSource;
use clap::Parser;
use wayland_client::Connection;
use wayland_client::globals::registry_queue_init;

use crate::cli::{Cli, Command, PluginAction};
use crate::session::{backend, ipc};
use crate::wayland::Shell;
use wayrun_ui::render;

fn main() -> process::ExitCode {
    let mut args = env::args();
    let invoked_as = args.next().unwrap_or_default();

    // A `wayrun-core` symlink keeps the documented stdin/JSON-RPC entry working.
    if Path::new(&invoked_as)
        .file_name()
        .is_some_and(|name| name == "wayrun-core")
    {
        return core();
    }

    let cli = Cli::parse();
    wayrun_core::locale::init();

    if let Some(script) = cli.lua_host.as_deref() {
        return match wayrun_core::lua::run(script) {
            Ok(()) => process::ExitCode::SUCCESS,
            Err(error) => {
                // `{:#}` keeps the cause: a broken script must say why.
                eprintln!("wayrun: {error:#}");
                process::ExitCode::FAILURE
            }
        };
    }
    if cli.core {
        return core();
    }

    match cli.command {
        Some(Command::Plugin { action }) => return plugin(action),
        Some(Command::Bench) => {
            render::bench();
            return process::ExitCode::SUCCESS;
        }
        Some(Command::Open) => return ipc::client("open"),
        Some(Command::Close) => return ipc::client("close"),
        Some(Command::Toggle) => return ipc::client("toggle"),
        Some(Command::Status) => return ipc::client("status"),
        None => {}
    }

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    match run() {
        Ok(()) => process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("wayrun: {error}");
            process::ExitCode::FAILURE
        }
    }
}

fn core() -> process::ExitCode {
    match wayrun_core::run() {
        Ok(()) => process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("wayrun: {error}");
            process::ExitCode::FAILURE
        }
    }
}

fn list_plugins() -> process::ExitCode {
    match wayrun_core::print_plugins() {
        Ok(()) => process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("wayrun: {error}");
            process::ExitCode::FAILURE
        }
    }
}

fn plugin(action: PluginAction) -> process::ExitCode {
    match action {
        PluginAction::List => list_plugins(),
        PluginAction::Check { script } => match wayrun_core::lua::check(&script) {
            Ok(true) => process::ExitCode::SUCCESS,
            Ok(false) => process::ExitCode::FAILURE,
            Err(error) => {
                eprintln!("wayrun: {error:#}");
                process::ExitCode::FAILURE
            }
        },
        PluginAction::Add {
            script,
            keyword,
            resident,
        } => add(&script, keyword, resident),
        PluginAction::Enable { id } => {
            outcome(wayrun_core::plugins::set_enabled(&id, true).map(|()| println!("enabled {id}")))
        }
        PluginAction::Disable { id } => outcome(
            wayrun_core::plugins::set_enabled(&id, false).map(|()| println!("disabled {id}")),
        ),
        PluginAction::Remove { id } => {
            outcome(wayrun_core::plugins::remove_plugin(&id).map(|()| println!("removed {id}")))
        }
    }
}

fn add(script: &str, keyword: Option<String>, resident: bool) -> process::ExitCode {
    let result = (|| -> anyhow::Result<()> {
        let plugins = wayrun_core::lua::plugin_ids(script)?;
        let [(id, name)] = plugins.as_slice() else {
            anyhow::bail!(
                "{script} declares {} plugins; register each one by hand",
                plugins.len()
            );
        };
        let keyword = keyword.unwrap_or_else(|| id.clone());
        wayrun_core::plugins::add_plugin(id, &keyword, Path::new(script), resident)?;
        println!("added {id} ({name}), keyword {keyword:?}");
        Ok(())
    })();
    outcome(result)
}

fn outcome(result: anyhow::Result<()>) -> process::ExitCode {
    match result {
        Ok(()) => process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("wayrun: {error:#}");
            process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let resident = env::var_os("WAYRUN_RESIDENT").is_some();

    // The IPC listener is also the single-instance guard: take it before the
    // first Wayland round trip and before any worker starts.
    let (ipc_tx, ipc_channel) = calloop::channel::channel();
    ipc::serve(ipc_tx)?;

    let conn = Connection::connect_to_env()?;
    let (globals, event_queue) = registry_queue_init::<Shell>(&conn)?;
    let qh = event_queue.handle();

    let mut event_loop: EventLoop<'static, Shell> = EventLoop::try_new()?;
    let loop_handle = event_loop.handle();

    let (backend_tx, backend_channel) = calloop::channel::channel();
    let (paste_tx, paste_channel) = calloop::channel::channel();
    let (icon_tx, icon_channel) = calloop::channel::channel();
    let (appearance_tx, appearance_channel) = calloop::channel::channel();

    loop_handle.insert_source(backend_channel, |event, _, state: &mut Shell| {
        if let ChannelEvent::Msg(event) = event {
            state.on_backend(event);
        }
    })?;
    loop_handle.insert_source(ipc_channel, |event, _, state: &mut Shell| {
        if let ChannelEvent::Msg(command) = event {
            state.on_ipc(command);
        }
    })?;
    loop_handle.insert_source(paste_channel, |event, _, state: &mut Shell| {
        if let ChannelEvent::Msg((generation, text)) = event {
            state.on_paste(generation, text);
        }
    })?;
    loop_handle.insert_source(icon_channel, |event, _, state: &mut Shell| {
        if let ChannelEvent::Msg((generation, key, icon)) = event {
            state.on_icon(generation, key, icon);
        }
    })?;
    loop_handle.insert_source(appearance_channel, |event, _, state: &mut Shell| {
        if let ChannelEvent::Msg(config) = event {
            state.on_appearance(config, Instant::now());
        }
    })?;
    WaylandSource::new(conn.clone(), event_queue).insert(loop_handle.clone())?;

    let icon_jobs = wayrun_ui::icons::spawn_worker(move |result| icon_tx.send(result).is_ok());
    let paste_jobs = session::clipboard::spawn(paste_tx);
    let mut shell = Shell::new(&conn, &qh, &loop_handle, &globals, paste_jobs, icon_jobs)?;
    shell.on_appearance(config::load(), Instant::now());
    // Held for the process's life: dropping it stops live config updates.
    let _appearance_watcher = config::watch(appearance_tx);

    backend::start(backend_tx);

    if !resident {
        shell.open(Instant::now());
    }

    while !shell.is_done() {
        event_loop.dispatch(None, &mut shell)?;
    }

    Ok(())
}
