use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

/// Every declared plugin's readable areas, by plugin id; filled after the
/// script is read, like the env names beside it.
pub(super) type Areas = Rc<RefCell<HashMap<String, Vec<PathBuf>>>>;

/// Refuses a path outside what the calling plugin may read; `fs.list`,
/// `fs.stat` and `sqlite.snapshot` run their argument through it.
pub(super) type Guard = Rc<dyn Fn(&str) -> Result<(), String>>;

/// A declared `~/…` or absolute path, made lexical: `~` expands, `.` drops,
/// and a relative name or a `..` climb is refused.
pub(super) fn expand(path: &str) -> Option<PathBuf> {
    let path = if path == "~" {
        crate::system::fs::get_home().ok()?
    } else if let Some(rest) = path.strip_prefix("~/") {
        crate::system::fs::get_home().ok()?.join(rest)
    } else if path.starts_with('/') {
        PathBuf::from(path)
    } else {
        return None;
    };
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::RootDir => out.push("/"),
            Component::Normal(name) => out.push(name),
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) => return None,
        }
    }
    Some(out)
}

/// `path` with symlinks followed as far as it exists; the rest stays lexical,
/// so a link inside an area cannot reach out of it.
fn resolve(path: &Path) -> PathBuf {
    let mut existing = path.to_path_buf();
    let mut rest = Vec::new();
    loop {
        if let Ok(resolved) = existing.canonicalize() {
            return rest
                .iter()
                .rev()
                .fold(resolved, |path, name| path.join(name));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                existing = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

pub(super) fn check(roots: &[PathBuf], requested: &str) -> Result<(), String> {
    let Some(path) = expand(requested) else {
        return Err("the name must be absolute or start with ~/".to_string());
    };
    let resolved = resolve(&path);
    if roots.iter().any(|root| resolved.starts_with(resolve(root))) {
        return Ok(());
    }
    Err("outside the areas the plugin declares".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_handles_tilde_and_refuses_climbs() {
        let home = crate::system::fs::get_home().unwrap();
        assert_eq!(expand("~/x//y").unwrap(), home.join("x").join("y"));
        assert_eq!(expand("~").unwrap(), home);
        assert_eq!(expand("/a/./b").unwrap(), PathBuf::from("/a/b"));
        assert!(expand("relative/x").is_none());
        assert!(expand("~/../x").is_none());
        assert!(expand("~root/x").is_none());
    }

    #[test]
    fn an_area_covers_its_tree_but_not_a_sibling() {
        let dir = tempfile::tempdir().unwrap();
        let roots = vec![dir.path().to_path_buf()];
        let inside = dir.path().join("a/b");
        assert!(check(&roots, inside.to_str().unwrap()).is_ok());
        assert!(check(&roots, "/etc/hostname").is_err());
        assert!(check(&roots, "nope").is_err());
    }

    #[test]
    fn a_symlink_cannot_step_out_of_an_area() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        std::fs::write(outside.path().join("note.txt"), "hi").unwrap();
        let roots = vec![dir.path().to_path_buf()];
        assert!(
            check(&roots, dir.path().join("link/note.txt").to_str().unwrap()).is_err(),
            "the link resolves outside"
        );
        assert!(
            check(&roots, dir.path().join("fresh.txt").to_str().unwrap()).is_ok(),
            "a missing name inside the area is still fine"
        );
    }
}
