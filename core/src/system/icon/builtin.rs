use std::path::Path;

/// UI glyphs compiled in and referenced as `builtin:<name>`, so panel, badges and
/// plugin identities never depend on a theme; the NOTICE sits beside them.
const BUILTIN_ICONS: &[(&str, &[u8])] = &[
    ("app", include_bytes!("../../../assets/icons/app.svg")),
    (
        "bookmark",
        include_bytes!("../../../assets/icons/bookmark.svg"),
    ),
    (
        "calculator",
        include_bytes!("../../../assets/icons/calculator.svg"),
    ),
    (
        "clipboard",
        include_bytes!("../../../assets/icons/clipboard.svg"),
    ),
    ("clock", include_bytes!("../../../assets/icons/clock.svg")),
    ("copy", include_bytes!("../../../assets/icons/copy.svg")),
    ("file", include_bytes!("../../../assets/icons/file.svg")),
    ("folder", include_bytes!("../../../assets/icons/folder.svg")),
    ("globe", include_bytes!("../../../assets/icons/globe.svg")),
    ("lock", include_bytes!("../../../assets/icons/lock.svg")),
    ("logout", include_bytes!("../../../assets/icons/logout.svg")),
    ("open", include_bytes!("../../../assets/icons/open.svg")),
    ("pin", include_bytes!("../../../assets/icons/pin.svg")),
    ("power", include_bytes!("../../../assets/icons/power.svg")),
    ("reboot", include_bytes!("../../../assets/icons/reboot.svg")),
    ("remove", include_bytes!("../../../assets/icons/remove.svg")),
    ("reveal", include_bytes!("../../../assets/icons/reveal.svg")),
    (
        "suspend",
        include_bytes!("../../../assets/icons/suspend.svg"),
    ),
    (
        "terminal",
        include_bytes!("../../../assets/icons/terminal.svg"),
    ),
    ("unpin", include_bytes!("../../../assets/icons/unpin.svg")),
    ("window", include_bytes!("../../../assets/icons/window.svg")),
];

/// Write `bytes` as `name` under `dir`, replacing a stale copy, and return the
/// path the shell reads; a glyph's bytes change with the binary, so compare first.
fn write_cached(dir: &Path, name: &str, bytes: &[u8]) -> Option<String> {
    std::fs::create_dir_all(dir).ok()?;
    let target = dir.join(name);
    if std::fs::read(&target).ok().as_deref() != Some(bytes) {
        crate::system::fs::write_atomic(&target, bytes).ok()?;
    }
    Some(target.to_string_lossy().into_owned())
}

/// A `builtin:<name>` glyph written into the cache.
pub(super) fn builtin_icon(name: &str) -> Option<String> {
    let (name, bytes) = BUILTIN_ICONS.iter().find(|(n, _)| *n == name)?;
    let dir = crate::system::fs::cache_dir()?.join("builtin");
    write_cached(&dir, &format!("{name}.svg"), bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stale_cached_glyph_is_replaced() {
        let dir = std::env::temp_dir().join(format!("wayrun-glyph-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pin.svg"), b"stale").unwrap();

        let path = write_cached(&dir, "pin.svg", b"fresh").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"fresh");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_builtin_is_a_colourful_svg() {
        for (name, bytes) in BUILTIN_ICONS {
            let text = std::str::from_utf8(bytes).unwrap();
            assert!(
                text.contains("<svg") && !text.contains("currentColor"),
                "{name} is not a colourful svg"
            );
        }
        assert!(builtin_icon("definitely-not-a-glyph").is_none());
    }

    #[test]
    fn the_notice_names_every_builtin() {
        let notice = include_str!("../../../assets/icons/NOTICE");
        for (name, _) in BUILTIN_ICONS {
            assert!(
                notice
                    .lines()
                    .any(|line| line.split_whitespace().next() == Some(*name)),
                "NOTICE does not name the {name} glyph"
            );
        }
    }
}
