use std::path::PathBuf;
use std::str::from_utf8;

use mlua::{Lua, Table, Value};

use super::options::{parse_options, with_params};
use super::{Client, Response, reply};
use crate::lua::sdk::now_seconds;
use crate::lua::{kv, warn};

#[derive(serde::Serialize, serde::Deserialize)]
struct Cached {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Client {
    /// The stored reply under `key`, or `None`; an unreadable entry reads as a
    /// miss, and the next store overwrites it.
    pub(super) fn cached(&self, key: &str) -> Option<Response> {
        let (id, path) = self.database()?;
        let text = match self.stores.borrow_mut().get(&id, &path, key, now_seconds()) {
            Ok(Some(text)) => text,
            Ok(None) => return None,
            Err(error) => {
                warn(&self.script, &format!("http cache read {key}: {error:#}"));
                return None;
            }
        };
        let cached: Cached = serde_json::from_str(&text).ok()?;
        Some(Response {
            status: cached.status,
            headers: cached.headers,
            body: cached.body.into_bytes(),
        })
    }

    /// The stored reply under `key` and whether it is stale; unlike `cached`,
    /// an expired entry still answers.
    fn cached_any(&self, key: &str) -> Option<(Response, bool)> {
        let (id, path) = self.database()?;
        let (text, stale) = match self
            .stores
            .borrow_mut()
            .get_any(&id, &path, key, now_seconds())
        {
            Ok(Some(found)) => found,
            Ok(None) => return None,
            Err(error) => {
                warn(&self.script, &format!("http cache read {key}: {error:#}"));
                return None;
            }
        };
        let cached: Cached = serde_json::from_str(&text).ok()?;
        Some((
            Response {
                status: cached.status,
                headers: cached.headers,
                body: cached.body.into_bytes(),
            },
            stale,
        ))
    }

    /// `wayrun.http.cached`: the stored reply alone, expired entries included,
    /// with `stale` marking one past its ttl.
    pub(super) fn cached_reply(
        &self,
        lua: &Lua,
        url: String,
        params: Option<Table>,
        options: Option<Value>,
    ) -> mlua::Result<Value> {
        let mut options = parse_options(lua, options, "get")?;
        let (url, mut headers) = with_params(url, params)?;
        headers.append(&mut options.headers);
        options.headers = headers;
        let key = cache_key(&url, &options.headers);
        let Some((response, stale)) = self.cached_any(&key) else {
            return Ok(Value::Nil);
        };
        let table = reply(lua, &response)?;
        table.set("stale", stale)?;
        Ok(Value::Table(table))
    }

    /// Caches a 2xx reply for `ttl` seconds under `key`; the cache is a
    /// convenience, so a failure is logged and the reply still answers.
    pub(super) fn store(&self, key: &str, response: &Response, ttl: u64) {
        let Some((id, path)) = self.database() else {
            return;
        };
        let Ok(body) = from_utf8(&response.body) else {
            return;
        };
        let cached = Cached {
            status: response.status,
            headers: response.headers.clone(),
            body: body.to_string(),
        };
        let Ok(text) = serde_json::to_string(&cached) else {
            return;
        };
        let expires_at = now_seconds() + ttl as i64;
        match self
            .stores
            .borrow_mut()
            .set(&id, &path, key, &text, Some(expires_at))
        {
            Ok(()) => {}
            Err(error) => warn(&self.script, &format!("http cache write {key}: {error:#}")),
        }
    }

    fn database(&self) -> Option<(String, PathBuf)> {
        match kv::database(&self.active, &self.paths) {
            Ok(found) => Some(found),
            Err(problem) => {
                warn(&self.script, &format!("http cache: {problem}"));
                None
            }
        }
    }
}

/// The cache key: the URL and the request headers, name-normalized and sorted,
/// so an authenticated or language-specific reply never answers another request.
pub(super) fn cache_key(url: &str, headers: &[(String, String)]) -> String {
    let mut pairs: Vec<(String, &str)> = headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.as_str()))
        .collect();
    pairs.sort();
    let mut key = format!("http:{url}");
    for (name, value) in pairs {
        key.push('\n');
        key.push_str(&name);
        key.push_str(": ");
        key.push_str(value);
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lua::http::test_support::{lua_with_http, read_request, serve, url_of};
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    #[test]
    fn the_cache_key_separates_requests_by_header() {
        let plain = cache_key("https://x/a", &[]);
        let auth = cache_key(
            "https://x/a",
            &[("Authorization".to_string(), "Bearer t".to_string())],
        );
        assert_ne!(plain, auth);
        assert_ne!(
            cache_key("https://x/a", &[("Accept".into(), "json".into())]),
            cache_key("https://x/a", &[("Accept".into(), "text".into())]),
        );
        // header order and name case do not split one request into two entries
        assert_eq!(
            cache_key(
                "https://x/a",
                &[("Accept".into(), "json".into()), ("X-A".into(), "1".into())]
            ),
            cache_key(
                "https://x/a",
                &[("x-a".into(), "1".into()), ("accept".into(), "json".into())]
            ),
        );
    }

    #[test]
    fn a_ttl_serves_the_second_call_from_the_store() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&seen);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let _ = read_request(&mut stream);
                let body = (counter.fetch_add(1, Ordering::SeqCst) + 1).to_string();
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let lua = lua_with_http(dir.path());
        let (first, second): (String, String) = lua
            .load(format!(
                r#"
                local first = http.get("{url}", nil, {{ ttl = 60 }})
                local second = http.get("{url}", nil, {{ ttl = 60 }})
                return first.body, second.body
                "#,
                url = url_of(port, "/count"),
            ))
            .eval()
            .unwrap();
        assert_eq!(first, "1");
        assert_eq!(second, "1", "the second call was served from the store");
        assert_eq!(
            seen.load(Ordering::SeqCst),
            1,
            "one request reached the server"
        );
        assert!(
            dir.path().join("demo/kv.db").is_file(),
            "the cache landed in the plugin's own store"
        );
    }

    #[test]
    fn cached_answers_from_the_store_without_the_network() {
        let (port, server) = serve(|mut stream| {
            let _ = read_request(&mut stream);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let lua = lua_with_http(dir.path());
        let (body, cached, stale, other_query, other_header): (
            String,
            Option<String>,
            bool,
            bool,
            bool,
        ) = lua
            .load(format!(
                r#"
                local url = "{url}"
                local got = http.get(url, nil, {{ ttl = 300 }})
                local hit = http.cached(url)
                return got.body,
                    hit and hit.body,
                    hit and hit.stale,
                    http.cached(url, {{ q = "other" }}) == nil,
                    http.cached(url, nil, {{ headers = {{ ["X-Other"] = "1" }} }}) == nil
                "#,
                url = url_of(port, "/cached"),
            ))
            .eval()
            .unwrap();
        server.join().unwrap();
        assert_eq!(
            (body.as_str(), cached.as_deref(), stale),
            ("ok", Some("ok"), false)
        );
        assert!(other_query && other_header);
    }
}
