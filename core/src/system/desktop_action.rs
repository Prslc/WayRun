use std::borrow::Cow;
use std::ffi::{CString, c_char};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use freedesktop_desktop_entry::DesktopEntry;
use gio::glib::translate::ToGlibPtr;

use crate::system::xdg;

// GLib's own action launcher honors DBusActivatable, Path, Terminal and the
// field codes; gio 0.22 binds none of it, so the two symbols are declared here.
#[link(name = "gio-2.0")]
unsafe extern "C" {
    fn g_desktop_app_info_new_from_filename(
        filename: *const c_char,
    ) -> *mut gio::glib::gobject_ffi::GObject;
    fn g_desktop_app_info_launch_action(
        info: *mut gio::glib::gobject_ffi::GObject,
        action_name: *const c_char,
        context: *mut gio::ffi::GAppLaunchContext,
    ) -> gio::glib::ffi::gboolean;
}

/// Every path a desktop id may live at, in XDG precedence order: the user's own
/// data dir first, then the padded `$XDG_DATA_DIRS`.
fn candidates(id: &str) -> Vec<PathBuf> {
    let names: Vec<String> = if id.ends_with(".desktop") {
        vec![id.to_owned()]
    } else {
        vec![format!("{id}.desktop")]
    };
    xdg::desktop_dirs()
        .into_iter()
        .flat_map(|root| names.iter().map(move |name| root.join(name)))
        .collect()
}

/// The most preferred `.desktop` file for `id`, user overrides included.
pub fn find(id: &str) -> Option<PathBuf> {
    if let Some(path) = candidates(id).into_iter().find(|path| path.is_file()) {
        return Some(path);
    }
    let wanted = if id.ends_with(".desktop") {
        id.to_owned()
    } else {
        format!("{id}.desktop")
    };
    xdg::desktop_dirs()
        .into_iter()
        .find_map(|root| find_nested(&root, &wanted))
}

/// A nested entry's id flattens its path (`foo/bar.desktop` -> `foo-bar.desktop`),
/// so an id no direct candidate serves is matched by walking the root.
fn find_nested(root: &Path, wanted: &str) -> Option<PathBuf> {
    for entry in walkdir::WalkDir::new(root).min_depth(1) {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("desktop") || !path.is_file() {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        if relative.to_string_lossy().replace('/', "-") == wanted {
            return Some(path.to_path_buf());
        }
    }
    None
}

/// The parsed entry for `id`, so every reader of `Icon=`/`GenericName=`/actions
/// reads the same file `launch:` would; `locales` selects localised keys.
pub fn entry(id: &str, locales: Option<&[String]>) -> Option<DesktopEntry> {
    let path = find(id)?;
    DesktopEntry::from_path(&path, locales).ok()
}

/// The locale list every `.desktop` read uses, from `g_get_language_names()`;
/// `.encoding` variants are dropped so `zh_CN` matches; rows and their `%c` agree.
pub fn locales() -> Vec<String> {
    locales_from(gio::glib::language_names().into_iter().map(Into::into))
}

fn locales_from(names: impl IntoIterator<Item = String>) -> Vec<String> {
    names
        .into_iter()
        .filter(|name| !name.contains('.'))
        .collect()
}

/// What a `.desktop` file says beyond what gio's `AppInfo` exposes, read in the
/// one pass that also keeps every reader on the same file.
pub struct Details {
    /// `GenericName=`, localised and trimmed; an empty one is dropped.
    pub generic: Option<String>,
    /// `Keywords=`, localised, trimmed, empties dropped.
    pub keywords: Vec<String>,
    /// `[Desktop Action …]` groups as `(id, localised name)`.
    pub actions: Vec<(String, String)>,
    /// The program the entry's `Exec=` runs, for the runner's PATH attribution.
    pub exec: Option<String>,
    /// `Terminal=`.
    pub terminal: bool,
}

/// Read `id`'s `.desktop` file and interpret what the launcher needs from it.
pub fn details(id: &str, locales: &[String]) -> Option<Details> {
    Some(details_of(&entry(id, Some(locales))?, locales))
}

fn details_of(entry: &DesktopEntry, locales: &[String]) -> Details {
    let generic = entry
        .generic_name(locales)
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty());

    let keywords = entry
        .keywords(locales)
        .unwrap_or_default()
        .into_iter()
        .map(|word| word.trim().to_string())
        .filter(|word| !word.is_empty())
        .collect();

    let actions = entry
        .actions()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|id| {
            // An action with no Exec can still be D-Bus activated.
            let executable = entry
                .action_exec(id)
                .is_some_and(|exec| !exec.trim().is_empty());
            if !executable && !entry.dbus_activatable() {
                return None;
            }
            let name = entry.action_name(id, locales)?.trim().to_string();
            (!name.is_empty()).then(|| (id.to_string(), name))
        })
        .collect();

    Details {
        generic,
        keywords,
        actions,
        exec: exec_basename(entry),
        terminal: entry.terminal(),
    }
}
/// Basename of the program a desktop entry's `Exec=` runs, for the runner's PATH hits.
fn exec_basename(entry: &DesktopEntry) -> Option<String> {
    let argv: Vec<String> = gio::glib::shell_parse_argv(entry.exec()?)
        .ok()?
        .iter()
        .map(|word| word.to_string_lossy().into_owned())
        .collect();
    exec_program(&argv)
}

/// The program a desktop `Exec=` runs, from its parsed argv: `env` is unwrapped
/// to the real program; a shell, sandbox or interpreter wrapper maps to nothing.
fn exec_program(argv: &[String]) -> Option<String> {
    let mut words = argv.iter();
    let first = program_name(words.next()?)?;
    let program = if first == "env" {
        let found = words.find(|word| !word.starts_with('-') && !word.contains('='))?;
        program_name(found)
    } else {
        Some(first)
    }?;
    const WRAPPERS: [&str; 8] = [
        "sh", "bash", "dash", "zsh", "flatpak", "snap", "python", "python3",
    ];
    (!WRAPPERS.contains(&program.as_str())).then_some(program)
}

/// The basename of a program word, e.g. `/usr/bin/foo` or `foo` -> `foo`.
fn program_name(word: &str) -> Option<String> {
    Path::new(word)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
}

/// The icon of the app a Wayland `app_id` names: the id's `.desktop` file wins
/// when one exists, otherwise the id is tried as a theme icon name.
pub fn icon_for_app_id(app_id: &str) -> Option<String> {
    if app_id.is_empty() {
        return None;
    }
    if let Some(icon) = entry(app_id, None).and_then(|e| e.icon().map(str::to_owned))
        && let Some(path) = crate::system::icon::resolve(&icon)
    {
        return Some(path);
    }
    crate::system::icon::resolve(app_id)
}

/// What this entry's field codes stand for. `%i` names the action group's own
/// `Icon=` when it has one, as the spec's action groups allow.
struct Codes<'a> {
    name: Option<Cow<'a, str>>,
    icon: Option<&'a str>,
    path: String,
}

impl<'a> Codes<'a> {
    fn of(entry: &'a DesktopEntry, action_id: &str, path: &Path, locales: &[String]) -> Self {
        Self {
            name: entry.name(locales),
            icon: entry
                .action_entry(action_id, "Icon")
                .or_else(|| entry.icon())
                .filter(|icon| !icon.trim().is_empty()),
            path: path.display().to_string(),
        }
    }

    /// One argument of the parsed `Exec=`, expanded into zero, one or two
    /// arguments of the argv. `%i` is the only code that becomes two.
    fn push(&self, arg: &str, argv: &mut Vec<String>) {
        if arg == "%i" {
            if let Some(icon) = self.icon {
                argv.push("--icon".to_owned());
                argv.push(icon.to_owned());
            }
            return;
        }
        let mut out = String::with_capacity(arg.len());
        let mut chars = arg.chars();
        while let Some(c) = chars.next() {
            if c != '%' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('%') => out.push('%'),
                Some('c') => out.push_str(self.name.as_deref().unwrap_or("")),
                Some('k') => out.push_str(&self.path),
                // `%f`/`%u`/`%F`/`%U` stand for a file this row does not carry
                // and `%i` was handled above; every other code drops out.
                Some(_) | None => {}
            }
        }
        // An argument that was nothing but field codes is gone, not empty.
        if !out.is_empty() {
            argv.push(out);
        }
    }
}

/// The argv of one `Exec=`, field codes expanded. The split is
/// `g_shell_parse_argv`, because the spec's quoting is not the shell's.
fn expand(exec: &str, entry: &DesktopEntry, action_id: &str, path: &Path) -> Vec<String> {
    let Ok(parsed) = gio::glib::shell_parse_argv(exec) else {
        return Vec::new();
    };
    let locales = locales();
    let codes = Codes::of(entry, action_id, path, &locales);
    let mut argv = Vec::with_capacity(parsed.len());
    for arg in parsed {
        codes.push(&arg.to_string_lossy(), &mut argv);
    }
    argv
}

/// Runs one action of one desktop entry; GLib's launcher first, with the hand
/// expanded `Exec=` as the fallback.
pub fn launch(desktop_id: &str, action_id: &str) -> bool {
    if launch_with_glib(desktop_id, action_id) {
        return true;
    }
    let Some(entry) = entry(desktop_id, None) else {
        return false;
    };
    let Some(exec) = entry.action_exec(action_id) else {
        return false;
    };
    let argv = expand(exec, &entry, action_id, &entry.path);
    if argv.is_empty() {
        return false;
    }
    let dir = entry.path().map(Path::new);
    if entry.terminal() {
        crate::system::executor::run_in_terminal_in(
            &crate::system::executor::shell_join(&argv),
            dir,
        );
    } else {
        crate::system::executor::execute_argv(&argv, dir);
    }
    true
}

fn launch_with_glib(desktop_id: &str, action_id: &str) -> bool {
    let Some(path) = find(desktop_id) else {
        return false;
    };
    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    let Ok(action) = CString::new(action_id) else {
        return false;
    };
    let context = crate::system::executor::launch_context();
    // SAFETY: both C strings outlive the calls; the context outlives the launch;
    // the returned info is unref'd exactly once.
    unsafe {
        let info = g_desktop_app_info_new_from_filename(path.as_ptr());
        if info.is_null() {
            return false;
        }
        let launched =
            g_desktop_app_info_launch_action(info, action.as_ptr(), context.to_glib_none().0) != 0;
        gio::glib::gobject_ffi::g_object_unref(info as *mut _);
        launched
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const ENTRY: &str = "\
[Desktop Entry]
Name=Nautilus
Icon=org.gnome.Nautilus
Exec=nautilus --new-window
Actions=new-window;tab;icon;

[Desktop Action new-window]
Name=New Window
Exec=nautilus --new-window %U

[Desktop Action tab]
Name=New Tab
Exec=nautilus --tab \"a b;c\"

[Desktop Action icon]
Name=Icon
Exec=nautilus %i --profile \"100%% sure\"
";

    fn argv(action_id: &str) -> Vec<String> {
        let path = Path::new("/usr/share/applications/org.gnome.Nautilus.desktop");
        let entry = DesktopEntry::from_str(path, ENTRY, None::<&[String]>).unwrap();
        expand(
            entry.action_exec(action_id).unwrap_or(""),
            &entry,
            action_id,
            path,
        )
    }

    #[test]
    fn an_action_takes_its_own_exec_and_loses_the_path_codes() {
        assert_eq!(
            argv("new-window"),
            ["nautilus", "--new-window"],
            "%U names a URI the row does not carry"
        );
        assert_eq!(argv("tab"), ["nautilus", "--tab", "a b;c"]);
    }

    #[test]
    fn a_nested_desktop_file_flattens_its_id() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("wine/Programs/app.desktop");
        fs::create_dir_all(nested.parent().unwrap()).unwrap();
        fs::write(&nested, "[Desktop Entry]\nName=App\nExec=/bin/true\n").unwrap();
        assert_eq!(
            find_nested(dir.path(), "wine-Programs-app.desktop"),
            Some(nested)
        );
        assert_eq!(find_nested(dir.path(), "app.desktop"), None);
    }

    #[test]
    fn a_dbus_activatable_action_needs_no_exec() {
        let details = details_of(
            "[Desktop Entry]\n\
             Name=App\n\
             DBusActivatable=true\n\
             Actions=Dbus;\n\
             \n\
             [Desktop Action Dbus]\n\
             Name=DBus Go\n",
            &["en"],
        );
        assert_eq!(
            details.actions,
            [("Dbus".to_string(), "DBus Go".to_string())]
        );
    }

    #[test]
    fn an_unknown_action_does_not_launch() {
        assert!(!launch("definitely-not-a-real-app.desktop", "go"));
    }

    #[test]
    fn an_empty_icon_is_not_passed_to_an_action() {
        let content = "[Desktop Entry]\n\
                       Name=App\n\
                       Exec=app\n\
                       Icon=\n\
                       Actions=go;\n\
                       [Desktop Action go]\n\
                       Name=Go\n\
                       Exec=app %i --x\n";
        let path = Path::new("/usr/share/applications/app.desktop");
        let entry = DesktopEntry::from_str(path, content, None::<&[String]>).unwrap();
        assert_eq!(
            expand(entry.action_exec("go").unwrap(), &entry, "go", path),
            ["app", "--x"]
        );
    }

    #[test]
    fn the_plain_entry_is_not_an_action() {
        assert!(argv("").is_empty());
        assert!(argv("does-not-exist").is_empty());
    }

    #[test]
    fn a_window_app_id_maps_to_its_desktop_icon() {
        // `firefox` is both an app_id and a desktop id; the entry's `Icon=` is
        // what the row should render.
        if find("firefox").is_none() {
            return; // no desktop entry on this machine
        }
        let path = icon_for_app_id("firefox").unwrap();
        assert!(path.starts_with('/'), "{path}");
    }

    #[test]
    fn a_quoted_argument_is_one_argument_and_a_percent_stays_literal() {
        assert_eq!(
            argv("icon"),
            [
                "nautilus",
                "--icon",
                "org.gnome.Nautilus",
                "--profile",
                "100% sure"
            ]
        );
    }

    #[test]
    fn a_semicolon_is_not_a_command_separator() {
        // The whole point of the argv runner: `;` is an argument, not syntax.
        assert_eq!(argv("tab")[2], "a b;c");
    }

    /// `details_of` against a `.desktop` body, with the locales pinned so the
    /// result cannot depend on the test runner's `$LANG`.
    fn details_of(content: &str, locales: &[&str]) -> Details {
        let locales: Vec<String> = locales.iter().map(|l| (*l).to_string()).collect();
        let path = Path::new("/usr/share/applications/app.desktop");
        let entry = DesktopEntry::from_str(path, content, Some(&locales)).unwrap();
        super::details_of(&entry, &locales)
    }

    #[test]
    fn details_read_the_generic_name_keywords_and_actions() {
        let details = details_of(
            "[Desktop Entry]\n\
             GenericName=Virtualization Software\n\
             Keywords=virtualization;\n\
             Actions=Manager;NoExec;\n\
             Name[de]=Oracle VirtualBox\n\
             \n\
             [Desktop Action Manager]\n\
             Name=Open VM Manager\n\
             Name[de]=VM Manager oeffnen\n\
             Exec=VirtualBox\n\
             \n\
             [Desktop Action NoExec]\n\
             Name=No Exec\n\
             \n\
             [Desktop Action Broken]\n\
             Name[de]=Nur auf Deutsch\n",
            &["en"],
        );
        assert_eq!(details.generic.as_deref(), Some("Virtualization Software"));
        assert_eq!(details.keywords, ["virtualization"]);
        // an undeclared group, a localised-only name and a missing Exec are not rows
        assert_eq!(
            details.actions,
            [("Manager".to_string(), "Open VM Manager".to_string())]
        );
    }

    #[test]
    fn a_localised_action_name_wins_in_its_locale() {
        let entry = "[Desktop Entry]\n\
                     Name=VirtualBox\n\
                     Actions=Manager;\n\
                     [Desktop Action Manager]\n\
                     Name=Open VM Manager\n\
                     Name[de]=VM Manager oeffnen\n\
                     Exec=VirtualBox\n";

        assert_eq!(
            details_of(entry, &["de_DE"]).actions[0].1,
            "VM Manager oeffnen"
        );
        assert_eq!(details_of(entry, &["en"]).actions[0].1, "Open VM Manager");
    }

    #[test]
    fn desktop_locales_drop_encoding_variants() {
        // the shape g_get_language_names returns for zh_CN.UTF-8, with the bare
        // language before an encoded key can match it
        let names = ["zh_CN.UTF-8", "zh_CN", "zh.UTF-8", "zh", "C"].map(String::from);
        assert_eq!(locales_from(names), ["zh_CN", "zh", "C"]);
        // a @modifier survives, its encoded spelling does not
        let names = ["sr_RS.UTF-8@latin", "sr_RS@latin", "sr@latin"].map(String::from);
        assert_eq!(locales_from(names), ["sr_RS@latin", "sr@latin"]);
    }

    #[test]
    fn an_env_wrapper_maps_to_the_real_program() {
        let argv = |words: &[&str]| {
            words
                .iter()
                .map(|word| (*word).to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            exec_program(&argv(&[
                "env",
                "GTK_IM_MODULE=fcitx",
                "nautilus",
                "--new-window"
            ])),
            Some("nautilus".into())
        );
        assert_eq!(
            exec_program(&argv(&["/usr/lib/firefox/firefox", "%u"])),
            Some("firefox".into())
        );
        assert_eq!(exec_program(&argv(&["sh", "-c", "scrcpy"])), None);
        assert_eq!(
            exec_program(&argv(&["env", "A=1", "flatpak", "run", "x"])),
            None
        );
    }
}
