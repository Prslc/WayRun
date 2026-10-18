use std::path::{Path, PathBuf};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Result, Watcher};

/// The paths to watch for a root: the nearest existing path, plus a file's
/// parent (atomic rename saves, which a file watch never reports).
pub(crate) fn root_targets(root: &Path) -> Vec<PathBuf> {
    let Some(existing) = root
        .ancestors()
        .find(|path| !path.as_os_str().is_empty() && path.exists())
    else {
        return Vec::new();
    };
    let mut targets = vec![existing.to_path_buf()];
    if existing.is_file()
        && let Some(parent) = existing.parent()
    {
        targets.push(parent.to_path_buf());
    }
    targets
}

fn watch_targets(watcher: &mut RecommendedWatcher, path: &Path) {
    for target in root_targets(path) {
        let _ = watcher.watch(&target, RecursiveMode::NonRecursive);
    }
}

/// Call `on_event` for every write to `path`; with no debounce, the handler must
/// keep its state on a failed read and ignore an unchanged value.
pub fn watch<F>(path: &Path, mut on_event: F) -> Option<RecommendedWatcher>
where
    F: FnMut() + Send + 'static,
{
    let watch_path: PathBuf = path.to_path_buf();
    let filter = watch_path.clone();
    let mut watcher = notify::recommended_watcher(move |res: Result<Event>| {
        let Ok(ev) = res else { return };
        // React to writes, not reads: the reload reads the file, so an
        // unfiltered Access event would re-trigger the watcher forever.
        if matches!(ev.kind, EventKind::Access(_)) {
            return;
        }
        if ev.paths.iter().any(|p| p == &filter) {
            on_event();
        }
    })
    .ok()?;

    watch_targets(&mut watcher, &watch_path);
    Some(watcher)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::root_targets;

    #[test]
    fn an_existing_directory_targets_itself() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(root_targets(temp.path()), vec![temp.path().to_path_buf()]);
    }

    #[test]
    fn a_missing_root_targets_its_nearest_existing_ancestor() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("gone").join("applications");
        assert_eq!(root_targets(&missing), vec![temp.path().to_path_buf()]);
    }

    #[test]
    fn an_existing_file_targets_itself_and_its_parent() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("entry.desktop");
        fs::write(&file, "").unwrap();
        assert_eq!(root_targets(&file), vec![file, temp.path().to_path_buf()]);
    }
}
