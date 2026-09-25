use std::path::Path;

use freedesktop_desktop_entry::DesktopEntry;

use super::model::{DesktopAction, DesktopMeta, Field};

/// Read `[Desktop Entry]`'s `GenericName`/`Keywords` and its `Actions=` groups,
/// localised through the file's own `Key[locale]=` entries.
pub(super) fn parse_meta(entry: &DesktopEntry, locales: &[String]) -> DesktopMeta {
    let generic = entry
        .generic_name(locales)
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .map(|name| Field::new(&name));

    let keywords = entry
        .keywords(locales)
        .unwrap_or_default()
        .into_iter()
        .map(|word| word.trim().to_string())
        .filter(|word| !word.is_empty())
        .map(|word| Field::new(&word))
        .collect();

    let actions = entry
        .actions()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|id| {
            let name = entry.action_name(id, locales)?.trim().to_string();
            (!name.is_empty()).then(|| DesktopAction {
                id: id.to_string(),
                name_lower: name.to_lowercase(),
                name,
            })
        })
        .collect();

    DesktopMeta {
        generic,
        keywords,
        actions,
    }
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

/// Basename of the program a desktop entry's `Exec=` runs, for the runner's PATH hits.
pub(super) fn exec_basename(entry: &DesktopEntry) -> Option<String> {
    let argv: Vec<String> = gio::glib::shell_parse_argv(entry.exec()?)
        .ok()?
        .iter()
        .map(|word| word.to_string_lossy().into_owned())
        .collect();
    exec_program(&argv)
}

/// The locale list gio localises `.desktop` keys with, from
/// `g_get_language_names()`; `.encoding` variants are dropped so `zh_CN` matches.
pub(super) fn desktop_locales() -> Vec<String> {
    desktop_locales_from(gio::glib::language_names().into_iter().map(Into::into))
}

fn desktop_locales_from(names: impl IntoIterator<Item = String>) -> Vec<String> {
    names
        .into_iter()
        .filter(|name| !name.contains('.'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `parse_meta` against a `.desktop` body, with the locales pinned so the
    /// result cannot depend on the test runner's `$LANG`.
    fn meta_of(content: &str, locales: &[&str]) -> DesktopMeta {
        let locales: Vec<String> = locales.iter().map(|l| (*l).to_string()).collect();
        let entry = DesktopEntry::from_str("app.desktop", content, Some(&locales)).unwrap();
        parse_meta(&entry, &locales)
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

    #[test]
    fn parse_meta_reads_action_groups() {
        let meta = meta_of(
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
        assert_eq!(
            meta.generic.as_ref().map(|g| g.lower.as_str()),
            Some("virtualization software")
        );
        assert_eq!(meta.keywords.len(), 1);
        assert_eq!(meta.keywords[0].lower, "virtualization");
        // an undeclared group is not a row, and neither is a localised-only name
        assert_eq!(meta.actions.len(), 1);
        assert_eq!(meta.actions[0].id, "Manager");
        assert_eq!(meta.actions[0].name, "Open VM Manager");
        assert_eq!(meta.actions[0].name_lower, "open vm manager");
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
            meta_of(entry, &["de_DE"]).actions[0].name,
            "VM Manager oeffnen"
        );
        assert_eq!(meta_of(entry, &["en"]).actions[0].name, "Open VM Manager");
    }

    #[test]
    fn desktop_locales_drop_encoding_variants() {
        // the shape g_get_language_names returns for zh_CN.UTF-8, with the bare
        // language before an encoded key can match it
        let names = ["zh_CN.UTF-8", "zh_CN", "zh.UTF-8", "zh", "C"].map(String::from);
        assert_eq!(desktop_locales_from(names), ["zh_CN", "zh", "C"]);
        // a @modifier survives, its encoded spelling does not
        let names = ["sr_RS.UTF-8@latin", "sr_RS@latin", "sr@latin"].map(String::from);
        assert_eq!(desktop_locales_from(names), ["sr_RS@latin", "sr@latin"]);
    }
}
