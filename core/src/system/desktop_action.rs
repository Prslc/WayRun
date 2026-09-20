use std::borrow::Cow;
use std::path::{Path, PathBuf};

use freedesktop_desktop_entry::{DesktopEntry, get_languages_from_env};

use crate::system::xdg;

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
    candidates(id).into_iter().find(|path| path.is_file())
}

/// The parsed entry for `id`, so every reader of `Icon=`/`GenericName=`/actions
/// reads the same file `launch:` would; `locales` selects localised keys.
pub fn entry(id: &str, locales: Option<&[String]>) -> Option<DesktopEntry> {
    let path = find(id)?;
    DesktopEntry::from_path(&path, locales).ok()
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
                .or_else(|| entry.icon()),
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
    let locales = get_languages_from_env();
    let codes = Codes::of(entry, action_id, path, &locales);
    let mut argv = Vec::with_capacity(parsed.len());
    for arg in parsed {
        codes.push(&arg.to_string_lossy(), &mut argv);
    }
    argv
}

/// The argv of `[Desktop Action <action_id>]`, or `None` when the entry carries
/// no such group or its `Exec=` expands to nothing.
pub fn action_argv(desktop_id: &str, action_id: &str) -> Option<Vec<String>> {
    let e = entry(desktop_id, None)?;
    let argv = expand(e.action_exec(action_id)?, &e, action_id, &e.path);
    (!argv.is_empty()).then_some(argv)
}

/// Runs one action of one desktop entry, returning the argv it ran.
pub fn launch(desktop_id: &str, action_id: &str) -> Option<Vec<String>> {
    let argv = action_argv(desktop_id, action_id)?;
    crate::system::executor::execute_argv(&argv);
    Some(argv)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
