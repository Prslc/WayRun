use crate::wire::SessionVerb;

use super::desktop_env;

const KDE_REBOOT: &str = concat!(
    "gdbus call --session --dest org.kde.Shutdown --object-path /Shutdown ",
    "--method org.kde.Shutdown.logoutAndReboot || systemctl reboot"
);
const KDE_SHUTDOWN: &str = concat!(
    "gdbus call --session --dest org.kde.Shutdown --object-path /Shutdown ",
    "--method org.kde.Shutdown.logoutAndShutdown || systemctl poweroff"
);
const KDE_LOGOUT: &str = concat!(
    "gdbus call --session --dest org.kde.Shutdown --object-path /Shutdown ",
    "--method org.kde.Shutdown.logout || loginctl terminate-session \"$XDG_SESSION_ID\""
);

/// The shell line that runs `verb` in this session.
pub fn command(verb: SessionVerb) -> &'static str {
    command_for(verb, desktop_env::id())
}

fn command_for(verb: SessionVerb, desktop: Option<&str>) -> &'static str {
    match (desktop, verb) {
        (Some("kde"), SessionVerb::Reboot) => KDE_REBOOT,
        (Some("kde"), SessionVerb::Shutdown) => KDE_SHUTDOWN,
        (Some("kde"), SessionVerb::Logout) => KDE_LOGOUT,
        _ => plain(verb),
    }
}

/// The plain command a non-KDE session runs.
fn plain(verb: SessionVerb) -> &'static str {
    match verb {
        SessionVerb::Lock => "loginctl lock-session",
        SessionVerb::Suspend => "systemctl suspend",
        SessionVerb::Reboot => "systemctl reboot",
        SessionVerb::Shutdown => "systemctl poweroff",
        SessionVerb::Logout => "loginctl terminate-session $XDG_SESSION_ID",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kde_session_asks_the_session_manager() {
        for (verb, command) in [
            (SessionVerb::Reboot, KDE_REBOOT),
            (SessionVerb::Shutdown, KDE_SHUTDOWN),
            (SessionVerb::Logout, KDE_LOGOUT),
        ] {
            assert_eq!(command_for(verb, Some("kde")), command);
        }
        assert!(KDE_REBOOT.contains("org.kde.Shutdown.logoutAndReboot"));
        assert!(KDE_REBOOT.ends_with("|| systemctl reboot"));
        assert!(KDE_SHUTDOWN.contains("org.kde.Shutdown.logoutAndShutdown"));
        assert!(KDE_SHUTDOWN.ends_with("|| systemctl poweroff"));
        assert!(KDE_LOGOUT.contains("org.kde.Shutdown.logout ||"));
        assert!(KDE_LOGOUT.ends_with("|| loginctl terminate-session \"$XDG_SESSION_ID\""));
    }

    #[test]
    fn other_sessions_keep_the_systemd_verbs() {
        for desktop in [None, Some("sway"), Some("niri")] {
            for (verb, command) in [
                (SessionVerb::Reboot, "systemctl reboot"),
                (SessionVerb::Shutdown, "systemctl poweroff"),
                (
                    SessionVerb::Logout,
                    "loginctl terminate-session $XDG_SESSION_ID",
                ),
            ] {
                assert_eq!(command_for(verb, desktop), command);
            }
        }
    }

    #[test]
    fn lock_and_suspend_ignore_the_session() {
        for desktop in [None, Some("kde")] {
            assert_eq!(
                command_for(SessionVerb::Lock, desktop),
                "loginctl lock-session"
            );
            assert_eq!(
                command_for(SessionVerb::Suspend, desktop),
                "systemctl suspend"
            );
        }
    }
}
