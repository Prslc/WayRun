mod app;
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
use wayland_client::Connection;
use wayland_client::globals::registry_queue_init;

use crate::session::{backend, ipc};
use crate::wayland::Shell;
use wayrun_ui::render;

fn main() -> process::ExitCode {
    wayrun_core::locale::init();
    let mut args = env::args();
    let invoked_as = args.next().unwrap_or_default();
    let rest: Vec<String> = args.collect();

    // The Lua plugin host: the VM lives in this same binary, never in the
    // core's process, and the script path follows the flag.
    if let Some(at) = rest.iter().position(|arg| arg == "--lua-host") {
        let script = rest.get(at + 1).map(String::as_str).unwrap_or_default();
        return match wayrun_core::lua::run(script) {
            Ok(()) => process::ExitCode::SUCCESS,
            Err(error) => {
                // `{:#}` keeps the cause: a broken script must say why.
                eprintln!("wayrun: {error:#}");
                process::ExitCode::FAILURE
            }
        };
    }

    // The shell re-execs this binary with `--core`; a `wayrun-core` symlink
    // keeps the documented stdin/JSON-RPC entry point working.
    let as_core = Path::new(&invoked_as)
        .file_name()
        .is_some_and(|name| name == "wayrun-core");
    if as_core
        || rest
            .iter()
            .any(|arg| arg == "--core" || arg == "--list-plugins")
    {
        return match wayrun_core::run() {
            Ok(()) => process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("wayrun: {error}");
                process::ExitCode::FAILURE
            }
        };
    }

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    if rest.first().map(String::as_str) == Some("bench") {
        render::bench();
        return process::ExitCode::SUCCESS;
    }

    if rest.first().map(String::as_str) == Some("plugin") {
        let script = (rest.get(1).map(String::as_str) == Some("check"))
            .then(|| rest.get(2))
            .flatten();
        let Some(script) = script else {
            eprintln!("usage: wayrun plugin check <script.lua>");
            return process::ExitCode::FAILURE;
        };
        return match wayrun_core::lua::check(script) {
            Ok(true) => process::ExitCode::SUCCESS,
            Ok(false) => process::ExitCode::FAILURE,
            Err(error) => {
                eprintln!("wayrun: {error:#}");
                process::ExitCode::FAILURE
            }
        };
    }

    if let Some(verb) = rest.first()
        && matches!(verb.as_str(), "open" | "close" | "toggle" | "status")
    {
        return ipc::client(verb);
    }

    match run() {
        Ok(()) => process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("wayrun: {error}");
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
