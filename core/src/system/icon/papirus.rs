use crate::system::xdg;

/// Papirus category dirs. Not every size ships every category: `panel` and
/// friends only exist in the small sizes, so lookup must scan sizes × categories.
const PAPIRUS_CATEGORIES: &[&str] = &[
    "actions",
    "apps",
    "categories",
    "devices",
    "emblems",
    "emotes",
    "mimetypes",
    "panel",
    "places",
    "status",
];
/// Preferred size order — the UI renders rows at 22–30px, so 48x48 gives crisp
/// downscale headroom without wasting load time on the 128px+ variants.
const PAPIRUS_SIZES: &[&str] = &[
    "48x48", "32x32", "64x64", "128x128", "96x96", "84x84", "42x42", "24x24", "22x22", "18x18",
    "16x16", "8x8",
];

/// `papirus:name` -> `(None, "name")`; `papirus:category/name` ->
/// `(Some("category"), "name")`.
fn parse_papirus_spec(spec: &str) -> (Option<&str>, &str) {
    match spec.split_once('/') {
        Some((cat, name)) => (Some(cat), name),
        None => (None, spec),
    }
}

pub(super) fn find_papirus(spec: &str) -> Option<String> {
    // `papirus:symbolic/[<category>/]<name>` scans the symbolic tree, which only
    // ships at the small sizes but is monochrome line art that tints cleanly.
    let (symbolic, spec) = match spec.strip_prefix("symbolic/") {
        Some(rest) => (true, rest),
        None => (false, spec),
    };
    let (hint, name) = parse_papirus_spec(spec);

    // category hint first when it names a real Papirus category, then the rest
    // (some names live under multiple categories)
    let mut categories: Vec<&str> = Vec::with_capacity(PAPIRUS_CATEGORIES.len() + 1);
    if let Some(h) = hint.filter(|h| PAPIRUS_CATEGORIES.contains(h)) {
        categories.push(h);
    }
    categories.extend(
        PAPIRUS_CATEGORIES
            .iter()
            .copied()
            .filter(|c| Some(*c) != hint),
    );

    let bases = xdg::icon_theme_dirs();

    // Cartesian scan in base × size × category order; first existing file wins.
    for base in &bases {
        for size in PAPIRUS_SIZES.iter().copied() {
            for category in categories.iter().copied() {
                let mut dir = base.join("Papirus").join(size);
                if symbolic {
                    dir = dir.join("symbolic");
                }
                let path = dir.join(category).join(format!("{name}.svg"));
                if path.exists() {
                    return Some(path.to_string_lossy().into_owned());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::icon::find_icon_path;
    use std::path::Path;

    #[test]
    fn papirus_spec_splits_category_hint() {
        assert_eq!(parse_papirus_spec("folder-open"), (None, "folder-open"));
        assert_eq!(
            parse_papirus_spec("panel/system-shutdown"),
            (Some("panel"), "system-shutdown")
        );
    }

    #[test]
    fn papirus_resolves_to_installed_path() {
        if !Path::new("/usr/share/icons/Papirus").exists() {
            return; // theme not installed on this machine
        }
        let path = find_icon_path("papirus:folder-open").unwrap();
        assert!(path.contains("/Papirus/"));
        assert!(path.ends_with(".svg"));
    }

    #[test]
    fn papirus_category_hint_resolves() {
        if !Path::new("/usr/share/icons/Papirus").exists() {
            return;
        }
        // `system-shutdown` also lives under `apps`; the hint scopes to `panel`
        // first, then falls back to the other categories.
        let path = find_icon_path("papirus:apps/system-shutdown").unwrap();
        assert!(path.contains("/apps/system-shutdown.svg"));
    }

    #[test]
    fn a_symbolic_spec_resolves_under_the_symbolic_tree() {
        if !Path::new("/usr/share/icons/Papirus").exists() {
            return;
        }
        // the monochrome line-art variant, which the panel tints to the fg
        let path = find_icon_path("papirus:symbolic/apps/utilities-terminal-symbolic").unwrap();
        assert!(path.contains("/symbolic/"), "{path}");
        assert!(path.ends_with("utilities-terminal-symbolic.svg"), "{path}");
    }

    #[test]
    fn papirus_unknown_name_falls_back_to_default() {
        let path = find_icon_path("papirus:definitely-not-an-icon-xyz");
        assert!(path.is_some()); // default icon, same semantics as any miss
    }
}
