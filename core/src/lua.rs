mod areas;
mod crypto;
mod fuzzy;
mod host;
mod http;
mod kv;
mod manifest;
mod sandbox;
mod sdk;
mod sqlite;

pub use host::run;

pub(super) fn warn(script: &str, message: &str) {
    eprintln!("wayrun-core: lua plugin {script}: {message}");
}
