use std::env;
use std::sync::OnceLock;

/// The desktop session this process runs in: a lowercase id such as `kde` or
/// `niri`, cached for the process's life. `None` when nothing names a session.
pub fn id() -> Option<&'static str> {
    static DETECTED: OnceLock<Option<String>> = OnceLock::new();
    DETECTED
        .get_or_init(|| classify(|name| env::var(name).ok()))
        .as_deref()
}

pub(crate) fn is(name: &str) -> bool {
    id().is_some_and(|id| id.eq_ignore_ascii_case(name))
}

/// The classification over an injected environment, so tests need not mutate
/// the process's own.
fn classify(read: impl Fn(&str) -> Option<String>) -> Option<String> {
    let mut names: Vec<String> = read("XDG_CURRENT_DESKTOP")
        .iter()
        .flat_map(|value| value.split(':'))
        .filter_map(normalized)
        .collect();
    names.extend(
        ["XDG_SESSION_DESKTOP", "DESKTOP_SESSION"]
            .into_iter()
            .filter_map(|var| read(var).as_deref().and_then(normalized)),
    );

    // A known name beats a conflicting marker, and a marker beats an unknown
    // label, so a distro tag cannot mask the session it runs.
    if let Some(id) = names.iter().find_map(|name| canonical(name)) {
        return Some(id.to_string());
    }
    marker(&read)
        .map(str::to_owned)
        .or_else(|| names.into_iter().next())
}

/// A session name reduced to the form the alias table matches: a path becomes
/// its file name, `.desktop` is dropped and the rest is ASCII-lowercased.
fn normalized(raw: &str) -> Option<String> {
    let raw = raw.trim();
    // A newline in a value would break the line-oriented status reply.
    if raw.is_empty() || raw.chars().any(char::is_control) {
        return None;
    }
    let name = raw.rsplit('/').next().unwrap_or(raw);
    let name = name.strip_suffix(".desktop").unwrap_or(name);
    (!name.is_empty()).then(|| name.to_ascii_lowercase())
}

fn canonical(name: &str) -> Option<&'static str> {
    match name {
        "kde" | "plasma" | "plasmawayland" | "plasma-wayland" | "kde-plasma" => Some("kde"),
        "niri" => Some("niri"),
        "hyprland" => Some("hyprland"),
        "sway" => Some("sway"),
        "river" => Some("river"),
        "wayfire" => Some("wayfire"),
        "labwc" => Some("labwc"),
        "mir" => Some("mir"),
        _ => None,
    }
}

fn marker(read: &impl Fn(&str) -> Option<String>) -> Option<&'static str> {
    let set = |name: &str| read(name).is_some_and(|value| !value.trim().is_empty());
    if read("KDE_FULL_SESSION").is_some_and(|value| value.trim().eq_ignore_ascii_case("true"))
        || set("KDE_SESSION_VERSION")
    {
        return Some("kde");
    }
    if set("NIRI_SOCKET") {
        return Some("niri");
    }
    if set("HYPRLAND_INSTANCE_SIGNATURE") {
        return Some("hyprland");
    }
    if set("SWAYSOCK") {
        return Some("sway");
    }
    if set("WAYFIRE_SOCKET") {
        return Some("wayfire");
    }
    None
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn detect(vars: &[(&str, &str)]) -> Option<String> {
        let vars: HashMap<&str, &str> = vars.iter().copied().collect();
        classify(|name| vars.get(name).map(|value| (*value).to_string()))
    }

    #[test]
    fn a_kde_desktop_name_is_kde() {
        assert_eq!(
            detect(&[("XDG_CURRENT_DESKTOP", "KDE")]).as_deref(),
            Some("kde")
        );
    }

    #[test]
    fn the_first_known_token_in_a_colon_list_wins() {
        assert_eq!(
            detect(&[("XDG_CURRENT_DESKTOP", "KDE:sway")]).as_deref(),
            Some("kde")
        );
        assert_eq!(
            detect(&[("XDG_CURRENT_DESKTOP", "sway:KDE")]).as_deref(),
            Some("sway")
        );
        assert_eq!(
            detect(&[("XDG_CURRENT_DESKTOP", "some-label:sway")]).as_deref(),
            Some("sway")
        );
    }

    #[test]
    fn the_session_variables_name_a_desktop() {
        assert_eq!(
            detect(&[("XDG_SESSION_DESKTOP", "plasma")]).as_deref(),
            Some("kde")
        );
        assert_eq!(
            detect(&[("DESKTOP_SESSION", "plasmawayland")]).as_deref(),
            Some("kde")
        );
        assert_eq!(
            detect(&[(
                "DESKTOP_SESSION",
                "/usr/share/wayland-sessions/plasma.desktop"
            )])
            .as_deref(),
            Some("kde")
        );
        assert_eq!(
            detect(&[
                ("XDG_SESSION_DESKTOP", "sway"),
                ("DESKTOP_SESSION", "plasma")
            ])
            .as_deref(),
            Some("sway")
        );
    }

    #[test]
    fn an_empty_desktop_name_falls_through() {
        assert_eq!(
            detect(&[
                ("XDG_CURRENT_DESKTOP", " : "),
                ("XDG_SESSION_DESKTOP", "KDE")
            ])
            .as_deref(),
            Some("kde")
        );
        assert_eq!(detect(&[("XDG_CURRENT_DESKTOP", ":")]), None);
    }

    #[test]
    fn markers_name_a_session_when_no_variable_does() {
        for (name, value, id) in [
            ("KDE_FULL_SESSION", "true", "kde"),
            ("KDE_SESSION_VERSION", "6", "kde"),
            ("NIRI_SOCKET", "/run/user/1000/niri.sock", "niri"),
            ("HYPRLAND_INSTANCE_SIGNATURE", "abc", "hyprland"),
            ("SWAYSOCK", "/run/user/1000/sway-ipc.sock", "sway"),
            ("WAYFIRE_SOCKET", "/tmp/wayfire.sock", "wayfire"),
        ] {
            assert_eq!(detect(&[(name, value)]).as_deref(), Some(id), "{name}");
        }
        assert_eq!(detect(&[("KDE_FULL_SESSION", "false")]), None);
        assert_eq!(detect(&[("KDE_SESSION_VERSION", "")]), None);
    }

    #[test]
    fn a_known_name_beats_a_conflicting_marker() {
        assert_eq!(
            detect(&[
                ("XDG_CURRENT_DESKTOP", "niri"),
                ("KDE_FULL_SESSION", "true")
            ])
            .as_deref(),
            Some("niri")
        );
    }

    #[test]
    fn a_marker_beats_an_unknown_name() {
        assert_eq!(
            detect(&[("XDG_CURRENT_DESKTOP", "some-label"), ("NIRI_SOCKET", "x")]).as_deref(),
            Some("niri")
        );
    }

    #[test]
    fn an_unknown_name_is_returned_lowercased() {
        assert_eq!(
            detect(&[("XDG_CURRENT_DESKTOP", "Unity")]).as_deref(),
            Some("unity")
        );
    }

    #[test]
    fn a_control_character_drops_the_value() {
        assert_eq!(detect(&[("XDG_CURRENT_DESKTOP", "KDE\nGNOME")]), None);
    }

    #[test]
    fn nothing_names_a_session() {
        assert_eq!(detect(&[]), None);
    }
}
