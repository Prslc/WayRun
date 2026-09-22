/// The environment variables that name a locale, most specific first.
const VARS: [&str; 4] = ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"];

/// Point the process at the UI locale. The core and the shell both call this at
/// startup, and the setting is process-global, so a row's action titles and the
/// shell's own chrome cannot end up in different languages.
pub fn init() {
    rust_i18n::set_locale(&detect());
}

/// The locale from the environment, normalised to a `locales/*.yml` stem:
/// `zh_CN.UTF-8` -> `zh_cn`. A locale with no table of its own (`en_US`) reads
/// `en` through the crate's fallback.
pub fn detect() -> String {
    detect_from(|key| std::env::var(key).ok())
}

fn detect_from(get: impl Fn(&str) -> Option<String>) -> String {
    let raw = VARS
        .iter()
        .find_map(|key| get(key).filter(|value| !value.trim().is_empty()))
        .unwrap_or_default();
    // `LANGUAGE` is a colon-separated list, and any variant may carry a codeset
    // or a modifier, so keep the language tag only.
    let tag = raw
        .split([':', '.', '@'])
        .next()
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    if tag.is_empty() || tag == "c" || tag == "posix" {
        return "en".to_string();
    }
    // Every Chinese variant reads the one shipped table rather than English.
    if tag.split('_').next() == Some("zh") {
        return "zh_cn".to_string();
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
        assert_eq!(detect_from(env(&[("LANG", "zh_CN.UTF-8")])), "zh_cn");
        assert_eq!(detect_from(env(&[("LANG", "en_US.UTF-8")])), "en_us");
        assert_eq!(detect_from(env(&[("LANG", "zh_TW.UTF-8")])), "zh_cn");
    }

    #[test]
    fn the_most_specific_variable_wins() {
        assert_eq!(
            detect_from(env(&[("LC_ALL", "zh_CN.UTF-8"), ("LANG", "en_US.UTF-8")])),
            "zh_cn"
        );
        assert_eq!(
            detect_from(env(&[("LANG", "en_US.UTF-8"), ("LANGUAGE", "zh_CN:en")])),
            "en_us"
        );
        assert_eq!(detect_from(env(&[("LANGUAGE", "zh_CN:en")])), "zh_cn");
        assert_eq!(
            detect_from(env(&[("LC_ALL", "  "), ("LANG", "zh_CN.UTF-8")])),
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
        assert_eq!(detect_from(env(&[])), "en");
        assert_eq!(detect_from(env(&[("LANG", "C")])), "en");
        assert_eq!(detect_from(env(&[("LC_ALL", "POSIX")])), "en");
    }
}
