use std::sync::OnceLock;

/// The environment variables that name a locale, most specific first.
const VARS: [&str; 4] = ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"];

/// The `LANGUAGE` the process started with, kept so launched apps can get it
/// back: `init` pins the launcher's own choice there.
static AMBIENT_LANGUAGE: OnceLock<Option<String>> = OnceLock::new();

/// Point the process at the UI locale (`ui.locale`, else the session's), for every
/// crate's chrome, the rows and glib's `.desktop` strings alike; needs a restart.
pub fn init() {
    let config = crate::config::get();
    let locale = resolve(&config.ui.locale, |key| std::env::var(key).ok());
    rust_i18n::set_locale(&locale);
    let ambient = std::env::var("LANGUAGE").ok();
    let tag = glib_tag(&locale);
    if ambient.as_deref() != Some(tag.as_str()) {
        // SAFETY: both binaries call `init` before any other thread exists, and
        // glib first reads the variable when its first locale lookup runs later.
        unsafe { std::env::set_var("LANGUAGE", &tag) };
    }
    let _ = AMBIENT_LANGUAGE.set(ambient);
}

/// The `LANGUAGE` the process started with; absent when the session set none.
pub(crate) fn ambient_language() -> Option<String> {
    AMBIENT_LANGUAGE.get().cloned().flatten()
}

/// The configured locale when it names one, else the session's.
fn resolve(configured: &str, get: impl Fn(&str) -> Option<String>) -> String {
    normalize(configured).unwrap_or_else(|| from_env(get))
}

/// The locale the session names, most specific variable first; `en` when it names none.
fn from_env(get: impl Fn(&str) -> Option<String>) -> String {
    VARS.iter()
        .find_map(|key| get(key).filter(|value| !value.trim().is_empty()))
        .as_deref()
        .and_then(normalize)
        .unwrap_or_else(|| "en".to_string())
}

/// A locale tag as a `locales/*.yml` stem: `zh_CN.UTF-8` -> `zh_cn`, `C`/`POSIX`
/// -> `en`; a locale with no table of its own reads `en` through the fallback.
fn normalize(raw: &str) -> Option<String> {
    // `LANGUAGE` is a colon-separated list, and any variant may carry a codeset
    // or a modifier, so keep the language tag only.
    let tag = raw
        .split([':', '.', '@'])
        .next()
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    if tag.is_empty() {
        return None;
    }
    if tag == "c" || tag == "posix" {
        return Some("en".to_string());
    }
    // Every Chinese variant reads the one shipped table rather than English.
    if tag.split('_').next() == Some("zh") {
        return Some("zh_cn".to_string());
    }
    Some(tag)
}

/// A locale stem as the language tag glib matches `.desktop` keys with:
/// `zh_cn` -> `zh_CN`, `en_us` -> `en_US`, `en` -> `en`.
fn glib_tag(stem: &str) -> String {
    let mut parts = stem.split('_');
    let mut tag = parts.next().unwrap_or_default().to_string();
    for part in parts {
        tag.push('_');
        tag.push_str(&part.to_uppercase());
    }
    tag
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        move |key| {
            pairs
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
        }
    }

    #[test]
    fn a_locale_becomes_a_file_stem() {
        assert_eq!(normalize("zh_CN.UTF-8").as_deref(), Some("zh_cn"));
        assert_eq!(normalize("en_US.UTF-8").as_deref(), Some("en_us"));
        assert_eq!(normalize("zh_TW.UTF-8").as_deref(), Some("zh_cn"));
        assert_eq!(normalize("  "), None);
    }

    #[test]
    fn a_stem_becomes_a_glib_language_tag() {
        assert_eq!(glib_tag("zh_cn"), "zh_CN");
        assert_eq!(glib_tag("en_us"), "en_US");
        assert_eq!(glib_tag("en"), "en");
    }

    #[test]
    fn the_most_specific_variable_wins() {
        assert_eq!(
            from_env(env(&[("LC_ALL", "zh_CN.UTF-8"), ("LANG", "en_US.UTF-8")])),
            "zh_cn"
        );
        assert_eq!(
            from_env(env(&[("LANG", "en_US.UTF-8"), ("LANGUAGE", "zh_CN:en")])),
            "en_us"
        );
        assert_eq!(from_env(env(&[("LANGUAGE", "zh_CN:en")])), "zh_cn");
        assert_eq!(
            from_env(env(&[("LC_ALL", "  "), ("LANG", "zh_CN.UTF-8")])),
            "zh_cn"
        );
    }

    fn workspace() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the core lives in the workspace")
            .to_path_buf()
    }

    fn sources(root: &Path) -> Vec<PathBuf> {
        walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "rs"))
            .map(|entry| entry.into_path())
            .collect()
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
    }

    /// The flat `a.b.c:` keys a locale file defines.
    fn locale_keys(path: &Path) -> BTreeSet<String> {
        read(path)
            .lines()
            .filter_map(|line| line.split_once(':'))
            .map(|(key, _)| key.trim().to_string())
            .filter(|key| key.contains('.') && !key.starts_with('#'))
            .collect()
    }

    /// Every literal key a `t!(…)` call names, in either crate.
    fn t_keys() -> BTreeSet<String> {
        let root = workspace();
        let mut keys = BTreeSet::new();
        for dir in [root.join("core/src"), root.join("shell/src")] {
            for file in sources(&dir) {
                let text = read(&file);
                let mut rest = text.as_str();
                while let Some(at) = rest.find("t!(\"") {
                    // `format!("` ends in the same four characters, so the `t`
                    // has to stand on its own.
                    let standalone = rest[..at]
                        .chars()
                        .next_back()
                        .is_none_or(|c| !c.is_alphanumeric() && c != '_');
                    rest = &rest[at + 4..];
                    if !standalone {
                        continue;
                    }
                    let Some(end) = rest.find('"') else { break };
                    keys.insert(rest[..end].to_string());
                    rest = &rest[end..];
                }
            }
        }
        keys
    }

    /// Every key a source tree names anywhere, whatever the call shape.
    fn named_keys(defined: &BTreeSet<String>) -> BTreeSet<String> {
        let root = workspace();
        let mut named = BTreeSet::new();
        for dir in [root.join("core/src"), root.join("shell/src")] {
            for file in sources(&dir) {
                let text = read(&file);
                for key in defined {
                    if text.contains(&format!("\"{key}\"")) {
                        named.insert(key.clone());
                    }
                }
            }
        }
        named
    }

    #[test]
    fn the_locales_define_the_same_keys() {
        let locales = workspace().join("locales");
        assert_eq!(
            locale_keys(&locales.join("en.yml")),
            locale_keys(&locales.join("zh_cn.yml"))
        );
    }

    #[test]
    fn every_key_a_t_call_names_is_defined() {
        let defined = locale_keys(&workspace().join("locales/en.yml"));
        let used = t_keys();
        // a scan that found nothing would make this vacuous
        assert!(used.len() > 20, "found only {} keys", used.len());
        let missing: Vec<&String> = used.difference(&defined).collect();
        assert!(missing.is_empty(), "used but undefined: {missing:?}");
    }

    #[test]
    fn no_key_is_left_unused() {
        let defined = locale_keys(&workspace().join("locales/en.yml"));
        let named = named_keys(&defined);
        let unused: Vec<&String> = defined.difference(&named).collect();
        assert!(unused.is_empty(), "defined but never named: {unused:?}");
    }

    #[test]
    fn the_c_locale_is_english() {
        assert_eq!(from_env(env(&[])), "en");
        assert_eq!(from_env(env(&[("LANG", "C")])), "en");
        assert_eq!(from_env(env(&[("LC_ALL", "POSIX")])), "en");
    }

    #[test]
    fn a_configured_locale_beats_the_session() {
        let session = env(&[("LANG", "en_US.UTF-8")]);
        assert_eq!(resolve("zh_cn", session), "zh_cn");
        assert_eq!(
            resolve("zh_CN.UTF-8", env(&[("LANG", "en_US.UTF-8")])),
            "zh_cn"
        );
        // English can be pinned over a Chinese session
        assert_eq!(resolve("en", env(&[("LANG", "zh_CN.UTF-8")])), "en");
        // a blank key is an absent one
        assert_eq!(resolve("  ", env(&[("LANG", "zh_CN.UTF-8")])), "zh_cn");
    }
}
