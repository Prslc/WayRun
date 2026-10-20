mod areas;
mod check;
mod crypto;
mod fuzzy;
mod host;
mod http;
mod kv;
mod log;
mod manifest;
mod module;
mod sandbox;
mod sdk;
mod sqlite;

pub use check::run as check;
pub use host::run;

pub(super) fn warn(script: &str, message: &str) {
    eprintln!("wayrun-core: lua plugin {script}: {message}");
}
