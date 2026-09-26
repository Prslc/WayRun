use std::path::{Path, PathBuf};

/// The Lua scripts the binary ships, embedded so a `script` entry needs no
/// install step; they are written into the cache where a path can name them.
const SHIPPED: &[(&str, &str)] = &[
    ("firefox.lua", include_str!("../../assets/lua/firefox.lua")),
    ("web.lua", include_str!("../../assets/lua/web.lua")),
];

/// The on-disk path of the shipped script `name`, refreshed when the binary's
/// copy moved on; an unknown name or a missing cache dir retires the entry.
pub fn path(name: &str) -> Option<PathBuf> {
    let (_, source) = SHIPPED.iter().find(|(shipped, _)| *shipped == name)?;
    write_into(&crate::system::fs::cache_dir()?.join("lua"), name, source)
}

fn write_into(dir: &Path, name: &str, source: &str) -> Option<PathBuf> {
    let path = dir.join(name);
    if std::fs::read_to_string(&path).ok().as_deref() == Some(source) {
        return Some(path);
    }
    std::fs::create_dir_all(dir).ok()?;
    crate::system::fs::write_atomic(&path, source.as_bytes()).ok()?;
    // The script runs through its shebang, so it needs the exec bit.
    let mut perms = std::fs::metadata(&path).ok()?.permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&path, perms).ok()?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shipped_script_lands_executable() {
        let dir = tempfile::tempdir().unwrap();
        let lua = dir.path().join("lua");
        let path = write_into(&lua, "firefox.lua", "return 1\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "return 1\n");
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(&path).unwrap().permissions(),
        );
        assert_eq!(mode & 0o111, 0o111, "a script needs the exec bit");
    }

    #[test]
    fn a_changed_script_replaces_the_cached_copy() {
        let dir = tempfile::tempdir().unwrap();
        let lua = dir.path().join("lua");
        write_into(&lua, "web.lua", "return 1\n").unwrap();
        let path = write_into(&lua, "web.lua", "return 2\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "return 2\n");
    }

    #[test]
    fn an_unknown_name_has_no_path() {
        assert!(path("not-shipped.lua").is_none());
    }
}
