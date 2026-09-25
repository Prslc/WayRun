use std::borrow::Cow;
use std::path::{Path, PathBuf};

use freedesktop_desktop_entry::DesktopEntry;

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

/// The locale list every `.desktop` read localises through, from
/// `g_get_language_names()`; `.encoding` variants are dropped so `zh_CN` matches.
/// One list for the whole core, so a row and the `%c` it launches with agree.
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
    let locales = locales();
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
             Actions=Manager;\n\
             Name[de]=Oracle VirtualBox\n\
             \n\
             [Desktop Action Manager]\n\
             Name=Open VM Manager\n\
             Name[de]=VM Manager oeffnen\n\
             Exec=VirtualBox\n\
             \n\
             [Desktop Action Broken]\n\
             Name[de]=Nur auf Deutsch\n",
            &["en"],
        );
        assert_eq!(details.generic.as_deref(), Some("Virtualization Software"));
        assert_eq!(details.keywords, ["virtualization"]);
        // an undeclared group is not a row, and neither is a localised-only name
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
                     Name[de]=VM Manager oeffnen\n";

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
