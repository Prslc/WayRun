use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use sha2::{Digest, Sha256};

const TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const FAIL_TTL: Duration = Duration::from_secs(5 * 60);
const ICON_MAX: u64 = 5 * 1024 * 1024;
const PREVIEW_MAX: u64 = 20 * 1024 * 1024;
const CACHE_BYTES: u64 = 128 * 1024 * 1024;
const CACHE_FILES: usize = 1024;
const QUEUE: usize = 64;
const WORKERS: usize = 4;
const PRUNE_EVERY: usize = 32;
const TIMEOUT: Duration = Duration::from_secs(5);

struct Job {
    url: String,
    path: PathBuf,
    max_bytes: u64,
}

#[derive(Default)]
struct State {
    wanted: HashSet<String>,
    in_flight: HashSet<String>,
    failed: HashMap<String, Instant>,
}

type Notifier = Arc<dyn Fn(&str) + Send + Sync>;

static STARTED: AtomicBool = AtomicBool::new(false);
static PRUNED: AtomicBool = AtomicBool::new(false);
static WRITTEN: AtomicUsize = AtomicUsize::new(0);
static JOBS: OnceLock<std::sync::mpsc::SyncSender<Job>> = OnceLock::new();
static NOTIFIER: OnceLock<Notifier> = OnceLock::new();
static STATE: LazyLock<Mutex<State>> = LazyLock::new(Default::default);

/// Start the background fetcher; idempotent, as the core starts once per process.
pub(crate) fn start(notify: impl Fn(&str) + Send + Sync + 'static) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let (jobs, queue) = std::sync::mpsc::sync_channel(QUEUE);
    let _ = JOBS.set(jobs);
    let notifier: Notifier = Arc::new(notify);
    let _ = NOTIFIER.set(notifier);
    let queue = Arc::new(Mutex::new(queue));
    for _ in 0..WORKERS {
        let queue = Arc::clone(&queue);
        std::thread::spawn(move || worker(queue));
    }
}

/// Invalidate queued fetches: only rows the newest search asked for still load.
pub(crate) fn supersede() {
    STATE.lock().expect("icon queue").wanted.clear();
}

/// A host's result icon: an `https://` URL becomes a stable local cache path and
/// queues a background fetch; other specs pass through untouched.
pub(crate) fn icon(spec: &str) -> Option<String> {
    url(spec, ICON_MAX)
}

/// A host's image preview: the same cache and queue, with a preview-sized cap.
pub(crate) fn preview(spec: &str) -> Option<String> {
    url(spec, PREVIEW_MAX)
}

fn url(spec: &str, max_bytes: u64) -> Option<String> {
    if !spec.starts_with("https://") {
        return None;
    }
    let path = cache_path(spec)?;
    if should_fetch(&path) && STARTED.load(Ordering::Relaxed) {
        enqueue(spec, path.clone(), max_bytes);
    }
    Some(path.display().to_string())
}

fn should_fetch(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return true;
    };
    meta.modified()
        .ok()
        .and_then(|mtime| mtime.elapsed().ok())
        .is_none_or(|age| age >= TTL)
}

fn enqueue(url: &str, path: PathBuf, max_bytes: u64) {
    let Some(jobs) = JOBS.get() else {
        return;
    };
    {
        let mut state = STATE.lock().expect("icon queue");
        if state
            .failed
            .get(url)
            .is_some_and(|at| at.elapsed() < FAIL_TTL)
        {
            return;
        }
        state.wanted.insert(url.to_string());
        if !state.in_flight.insert(url.to_string()) {
            return;
        }
    }
    let job = Job {
        url: url.to_string(),
        path,
        max_bytes,
    };
    if jobs.try_send(job).is_err() {
        STATE.lock().expect("icon queue").in_flight.remove(url);
    }
}

fn worker(queue: Arc<Mutex<std::sync::mpsc::Receiver<Job>>>) {
    if !PRUNED.swap(true, Ordering::Relaxed) {
        prune();
    }
    let agent = agent();
    loop {
        let job = {
            let queue = queue.lock().expect("icon queue");
            queue.recv()
        };
        let Ok(job) = job else {
            return;
        };
        {
            let mut state = STATE.lock().expect("icon queue");
            if !state.wanted.contains(&job.url) {
                state.in_flight.remove(&job.url);
                continue;
            }
        }
        let result = fetch(&agent, &job.url, &job.path, job.max_bytes);
        let mut state = STATE.lock().expect("icon queue");
        state.in_flight.remove(&job.url);
        match result {
            Ok(()) => {
                drop(state);
                if let Some(notify) = NOTIFIER.get() {
                    notify(&job.path.display().to_string());
                }
                if (WRITTEN.fetch_add(1, Ordering::Relaxed) + 1).is_multiple_of(PRUNE_EVERY) {
                    prune();
                }
            }
            Err(()) => {
                if state.failed.len() >= 4096 {
                    state.failed.clear();
                }
                state.failed.insert(job.url, Instant::now());
            }
        }
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .http_status_as_error(false)
            .proxy(ureq::Proxy::try_from_env())
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                    .build(),
            )
            .build(),
    )
}

fn fetch(agent: &ureq::Agent, url: &str, path: &Path, max_bytes: u64) -> Result<(), ()> {
    let mut response = agent
        .get(url)
        .header("User-Agent", "WayRun")
        .config()
        .timeout_global(Some(TIMEOUT))
        .build()
        .call()
        .map_err(|_| ())?;
    if !(200..300).contains(&response.status().as_u16()) {
        return Err(());
    }
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = response
        .body_mut()
        .with_config()
        .limit(max_bytes)
        .read_to_vec()
        .map_err(|_| ())?;
    if body.len() as u64 > max_bytes || !image(&content_type, &body) {
        return Err(());
    }
    write(path, &body).map_err(|_| ())
}

fn image(content_type: &str, body: &[u8]) -> bool {
    content_type.starts_with("image/")
        || body.starts_with(b"\x89PNG")
        || body.starts_with(&[0xff, 0xd8])
        || body.starts_with(b"GIF8")
        || body.starts_with(b"BM")
        || (body.starts_with(b"RIFF") && body.get(8..12) == Some(b"WEBP"))
        || body.starts_with(b"<svg")
        || body.starts_with(b"<?xml")
}

fn write(path: &Path, body: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        crate::system::fs::create_dir_private(dir)?;
    }
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, body)?;
    std::fs::rename(&temp, path)
}

fn cache_path(url: &str) -> Option<PathBuf> {
    let root = crate::system::fs::cache_root()?.join("icons");
    Some(root.join(format!("{}.{}", hash(url), extension(url))))
}

fn hash(url: &str) -> String {
    let digest = Sha256::digest(url.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest.iter().take(16) {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn extension(url: &str) -> &'static str {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => "png",
        "jpg" | "jpeg" => "jpg",
        "webp" => "webp",
        "gif" => "gif",
        "bmp" => "bmp",
        "svg" => "svg",
        _ => "img",
    }
}

/// Keep the disk cache bounded: oldest files go first once either cap is over.
fn prune() {
    let Some(dir) = crate::system::fs::cache_root().map(|root| root.join("icons")) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut files: Vec<(PathBuf, u64, SystemTime)> = entries
        .flatten()
        .filter_map(|entry| {
            if entry.path().extension().is_some_and(|ext| ext == "tmp") {
                return None;
            }
            let meta = entry.metadata().ok()?;
            meta.is_file()
                .then_some((entry.path(), meta.len(), meta.modified().ok()?))
        })
        .collect();
    if files.len() <= CACHE_FILES && files.iter().map(|(_, len, _)| len).sum::<u64>() <= CACHE_BYTES
    {
        return;
    }
    files.sort_by_key(|(_, _, mtime)| *mtime);
    let mut count = files.len();
    let mut total: u64 = files.iter().map(|(_, len, _)| *len).sum();
    for (path, len, _) in files {
        if count <= CACHE_FILES && total <= CACHE_BYTES {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            count -= 1;
            total = total.saturating_sub(len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_keeps_a_known_extension() {
        assert_eq!(extension("https://x/a.PNG?v=1"), "png");
        assert_eq!(extension("https://x/a.jpeg#frag"), "jpg");
        assert_eq!(extension("https://x/a.svg"), "svg");
        assert_eq!(extension("https://x/a?name=b.png"), "img");
        assert_eq!(extension("https://x/a"), "img");
    }

    #[test]
    fn the_cache_path_is_stable_and_https_only() {
        let a = cache_path("https://example.com/a.png").unwrap();
        let b = cache_path("https://example.com/a.png").unwrap();
        assert_eq!(a, b);
        assert!(a.to_string_lossy().contains("/icons/"));
        assert!(a.to_string_lossy().ends_with(".png"));
        assert!(icon("http://example.com/a.png").is_none());
        assert!(icon("builtin:app").is_none());
        assert!(preview("https://example.com/a.jpg").is_some());
        assert!(preview("http://example.com/a.jpg").is_none());
    }
}
