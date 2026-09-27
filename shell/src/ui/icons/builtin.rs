use wayrun_core::wire::BUILTIN_FALLBACK;

/// The glyphs compiled in from `assets/icons/`, keyed by the bare name a
/// `builtin:` spec carries; the NOTICE sits beside them.
pub(super) const GLYPHS: &[(&str, &[u8])] = &[
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

/// The bytes for `name`, falling back to the app glyph so every row draws one.
pub(super) fn glyph(name: &str) -> &'static [u8] {
    let bytes = |name: &str| GLYPHS.iter().find(|(n, _)| *n == name).map(|(_, b)| *b);
    let app = BUILTIN_FALLBACK
        .strip_prefix("builtin:")
        .unwrap_or(BUILTIN_FALLBACK);
    bytes(name).or_else(|| bytes(app)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use wayrun_core::wire::BUILTIN_GLYPHS;

    use super::*;

    #[test]
    fn every_glyph_is_a_colourful_svg() {
        for (name, bytes) in GLYPHS {
            let text = std::str::from_utf8(bytes).unwrap();
            assert!(
                text.contains("<svg") && !text.contains("currentColor"),
                "{name} is not a colourful svg"
            );
        }
    }

    #[test]
    fn an_unknown_name_falls_back_to_the_app_glyph() {
        assert_eq!(glyph("definitely-not-a-glyph"), glyph("app"));
    }

    #[test]
    fn every_wire_glyph_has_bytes() {
        for name in BUILTIN_GLYPHS {
            assert!(
                GLYPHS.iter().any(|(n, _)| n == name),
                "{name} has no compiled bytes"
            );
        }
    }

    #[test]
    fn the_notice_names_every_glyph() {
        let notice = include_str!("../../../assets/icons/NOTICE");
        for (name, _) in GLYPHS {
            assert!(
                notice
                    .lines()
                    .any(|line| line.split_whitespace().next() == Some(*name)),
                "NOTICE does not name the {name} glyph"
            );
        }
    }
}
