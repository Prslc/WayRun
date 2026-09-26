use anyhow::{Context, Result};
use std::env;
use std::ffi::OsStr;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Release the allocator's free pages back to the kernel. Only glibc's
/// `malloc_trim` does this; other targets leave it to their allocator.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub fn trim_allocator() {
    // SAFETY: `malloc_trim` is a plain libc allocator call with no preconditions
    // and no memory effects beyond returning free pages.
    unsafe {
        libc::malloc_trim(0);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub fn trim_allocator() {}

pub fn get_home() -> Result<PathBuf> {
    dirs::home_dir().context("finding the user HOME directory")
}

/// `$XDG_CACHE_HOME/wayrun` (or `~/.cache/wayrun`), created on demand.
pub fn cache_dir() -> Option<PathBuf> {
    let base = env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(env::temp_dir);
    let dir = base.join("wayrun");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// The first `$PATH` entry holding an executable `name`; a name containing a
/// slash is a path itself. Symlinks resolve to the target whose exec bit decides.
pub fn which_in(path: impl AsRef<OsStr>, name: &str) -> Option<PathBuf> {
    let executable = |candidate: &Path| {
        std::fs::metadata(candidate)
            .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    };
    if name.contains('/') {
        let candidate = PathBuf::from(name);
        return executable(&candidate).then_some(candidate);
    }
    env::split_paths(&path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(name))
        .find(|candidate| executable(candidate))
}

/// Create `path` only when it is absent; `O_EXCL` keeps the test atomic, so an
/// editor's atomic save is never truncated.
pub fn write_if_absent(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(contents.as_bytes())
}

/// The staging name [`write_atomic`] writes through, so anything cleaning up
/// after it (a `.tmp` a crash left behind) does not spell the rule its own way.
pub fn tmp_path(path: &Path) -> PathBuf {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    PathBuf::from(tmp)
}

/// Write `<path>.tmp`, then rename: a reader only ever sees a whole file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_atomic_with(path, |file| file.write_all(bytes))
}

/// [`write_atomic`] for a body a closure writes, so a caller holding large
/// pieces streams them instead of assembling the bytes first.
pub fn write_atomic_with(
    path: &Path,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let tmp = tmp_path(path);
    let mut file = std::fs::File::create(&tmp)?;
    write(&mut file)?;
    drop(file);
    std::fs::rename(&tmp, path)
}

/// Flatpak apps live in `<installation>/exports/share`, which only reaches
/// `XDG_DATA_DIRS` from a login shell; runs before GLib caches the dirs.
pub fn ensure_flatpak_data_dirs() {
    let home = env::var("HOME").unwrap_or_default();
    let dirs =
        env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    let missing: Vec<String> = [
        format!("{home}/.local/share/flatpak/exports/share"),
        "/var/lib/flatpak/exports/share".to_string(),
    ]
    .into_iter()
    .filter(|dir| PathBuf::from(dir).is_dir() && !dirs.split(':').any(|d| d == dir))
    .collect();

    if !missing.is_empty() {
        // SAFETY: `main` calls this before any other thread exists.
        unsafe { env::set_var("XDG_DATA_DIRS", format!("{}:{}", missing.join(":"), dirs)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_in_scans_the_path_and_checks_the_exec_bit() {
        let dir = tempfile::tempdir().unwrap();
        let write = |path: &Path, mode: u32| {
            std::fs::write(path, "#!/bin/sh\n").unwrap();
            let mut perms = std::fs::metadata(path).unwrap().permissions();
            perms.set_mode(mode);
            std::fs::set_permissions(path, perms).unwrap();
        };
        let tool = dir.path().join("tool");
        write(&tool, 0o755);
        let plain = dir.path().join("plain");
        write(&plain, 0o644);
        let path = dir.path().to_str().unwrap();

        assert_eq!(which_in(path, "tool").as_deref(), Some(tool.as_path()));
        assert_eq!(which_in(path, "plain"), None, "the exec bit decides");
        assert_eq!(which_in(path, "missing"), None);
        assert_eq!(
            which_in(path, tool.to_str().unwrap()).as_deref(),
            Some(tool.as_path()),
            "a name with a slash is a path"
        );
        assert_eq!(which_in(path, plain.to_str().unwrap()), None);

        let first = tempfile::tempdir().unwrap();
        write(&first.path().join("tool"), 0o755);
        let joined = format!("{}:{}", first.path().display(), dir.path().display());
        assert_eq!(
            which_in(&joined, "tool").as_deref(),
            Some(first.path().join("tool").as_path()),
            "the first entry wins"
        );
    }

    #[test]
    fn writing_a_missing_file_never_truncates_an_existing_one() {
        let dir =
            std::env::temp_dir().join(format!("wayrun-write-if-absent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("x.toml");

        write_if_absent(&path, "first").unwrap();
        assert!(write_if_absent(&path, "second").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
