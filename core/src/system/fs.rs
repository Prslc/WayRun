use anyhow::{Context, Result};
use std::env;
use std::io::Write;
use std::path::{Path, PathBuf};

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

/// Write `<path>.tmp`, then rename: a reader only ever sees a whole file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
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
