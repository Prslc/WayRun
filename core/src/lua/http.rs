use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use mlua::{Lua, LuaSerdeExt, Table, Value};

use super::kv;
use super::sdk::now_seconds;
use super::warn;

/// `wayrun.http`: the sandbox's one network exit. One agent per host keeps the
/// connection pool; every request carries its own deadline.
pub(super) fn lib(
    lua: &Lua,
    script: &str,
    stores: kv::Handle,
    active: kv::Active,
    paths: kv::Paths,
) -> mlua::Result<Table> {
    let client = Client {
        agent: agent(),
        script: script.to_string(),
        stores,
        active,
        paths,
    };
    let http = lua.create_table()?;
    let get = client.clone();
    http.set(
        "get",
        lua.create_function(
            move |lua, (url, params, options): (String, Option<Table>, Option<Value>)| {
                get.request(lua, false, url, params, options)
            },
        )?,
    )?;
    let post = client;
    http.set(
        "post",
        lua.create_function(
            move |lua, (url, params, options): (String, Option<Table>, Option<Value>)| {
                post.request(lua, true, url, params, options)
            },
        )?,
    )?;
    Ok(http)
}

fn agent() -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    let config = ureq::config::Config::builder()
        .http_status_as_error(false)
        .proxy(ureq::Proxy::try_from_env())
        .tls_config(tls)
        .build();
    ureq::Agent::new_with_config(config)
}

#[derive(Clone)]
struct Client {
    agent: ureq::Agent,
    script: String,
    stores: kv::Handle,
    active: kv::Active,
    paths: kv::Paths,
}

struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

#[derive(Default)]
struct Options {
    timeout_ms: Option<u64>,
    ttl: Option<u64>,
    headers: Vec<(String, String)>,
    body: Body,
}

#[derive(Default)]
enum Body {
    #[default]
    None,
    Json(String),
    Form(Vec<(String, String)>),
    Raw(Vec<u8>),
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Cached {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Client {
    fn request(
        &self,
        lua: &Lua,
        post: bool,
        url: String,
        params: Option<Table>,
        options: Option<Value>,
    ) -> mlua::Result<Table> {
        let method = if post { "post" } else { "get" };
        let mut options = parse_options(lua, options, method)?;
        let (url, mut headers) = with_params(url, params)?;
        headers.append(&mut options.headers);
        options.headers = headers;

        let ttl = if post {
            None
        } else {
            options.ttl.filter(|ttl| *ttl > 0)
        };
        if ttl.is_some()
            && let Some(cached) = self.cached(&url)
        {
            return reply(lua, &cached);
        }
        let response = self.perform(method, &url, &options, post)?;
        if let Some(ttl) = ttl
            && (200..300).contains(&response.status)
        {
            self.store(&url, &response, ttl);
        }
        reply(lua, &response)
    }

    fn perform(
        &self,
        method: &str,
        url: &str,
        options: &Options,
        post: bool,
    ) -> mlua::Result<Response> {
        let timeout = timeout_for(options.timeout_ms);
        let response = if post {
            let mut request = self.agent.post(url);
            for (name, value) in &options.headers {
                request = request.header(name.as_str(), value.as_str());
            }
            let request = request.config().timeout_global(Some(timeout)).build();
            match &options.body {
                Body::None => request.send_empty(),
                Body::Json(text) => request.content_type("application/json").send(text.clone()),
                Body::Form(pairs) => request.send_form(pairs.clone()),
                Body::Raw(bytes) => request.send(bytes.clone()),
            }
        } else {
            let mut request = self.agent.get(url);
            for (name, value) in &options.headers {
                request = request.header(name.as_str(), value.as_str());
            }
            request
                .config()
                .timeout_global(Some(timeout))
                .build()
                .call()
        };
        let mut response = response
            .map_err(|error| mlua::Error::RuntimeError(format!("http.{method} {url}: {error}")))?;
        let status = response.status().as_u16();
        let mut headers: BTreeMap<String, String> = BTreeMap::new();
        for (name, value) in response.headers() {
            let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
            headers
                .entry(name.as_str().to_string())
                .and_modify(|joined| {
                    joined.push_str(", ");
                    joined.push_str(&value);
                })
                .or_insert(value);
        }
        let body = response
            .body_mut()
            .read_to_vec()
            .map_err(|error| mlua::Error::RuntimeError(format!("http.{method} {url}: {error}")))?;
        Ok(Response {
            status,
            headers: headers.into_iter().collect(),
            body,
        })
    }

    /// The stored reply for `url`, or `None`; an unreadable entry reads as a
    /// miss, and the next store overwrites it.
    fn cached(&self, url: &str) -> Option<Response> {
        let (id, path) = self.database()?;
        let key = cache_key(url);
        let text = match self
            .stores
            .borrow_mut()
            .get(&id, &path, &key, now_seconds())
        {
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

    /// Caches a 2xx reply for `ttl` seconds under its URL; the cache is a
    /// convenience, so a failure is logged and the reply still answers.
    fn store(&self, url: &str, response: &Response, ttl: u64) {
        let Some((id, path)) = self.database() else {
            return;
        };
        let Ok(body) = std::str::from_utf8(&response.body) else {
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
        let key = cache_key(url);
        let expires_at = now_seconds() + ttl as i64;
        match self
            .stores
            .borrow_mut()
            .set(&id, &path, &key, &text, Some(expires_at))
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

fn reply(lua: &Lua, response: &Response) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("status", response.status)?;
    let headers = lua.create_table()?;
    for (name, value) in &response.headers {
        headers.set(name.as_str(), value.as_str())?;
    }
    table.set("headers", headers)?;
    table.set("body", lua.create_string(&response.body)?)?;
    Ok(table)
}

fn cache_key(url: &str) -> String {
    format!("http:{url}")
}

/// The deadline: two seconds by default, held inside the host call's own
/// ceiling so an overlong timeout fails here rather than as a killed host.
fn timeout_for(millis: Option<u64>) -> Duration {
    let ceiling =
        crate::provider::external::HOST_TIMEOUT.saturating_sub(Duration::from_millis(500));
    Duration::from_millis(millis.unwrap_or(2_000)).clamp(Duration::from_millis(100), ceiling)
}

/// The request URL with `params` appended, plus the headers those params may
/// carry under the reserved `headers` name the launcher's plugins already use.
fn with_params(
    url: String,
    params: Option<Table>,
) -> mlua::Result<(String, Vec<(String, String)>)> {
    let mut url = url;
    let mut headers = Vec::new();
    let Some(params) = params else {
        return Ok((url, headers));
    };
    let mut separator = if url.contains('?') { '&' } else { '?' };
    for pair in params.pairs::<String, Value>() {
        let (name, value) = pair?;
        if name == "headers" {
            headers = string_pairs(&value, "headers")?;
            continue;
        }
        let value = match value {
            Value::Integer(number) => number.to_string(),
            Value::Number(number) => number.to_string(),
            Value::String(text) => text.to_string_lossy(),
            Value::Boolean(flag) => flag.to_string(),
            other => {
                return Err(mlua::Error::RuntimeError(format!(
                    "http params must be scalars, got {other:?}"
                )));
            }
        };
        url.push(separator);
        separator = '&';
        url.push_str(&urlencoding::encode(&name));
        url.push('=');
        url.push_str(&urlencoding::encode(&value));
    }
    Ok((url, headers))
}

fn parse_options(lua: &Lua, value: Option<Value>, method: &str) -> mlua::Result<Options> {
    let mut options = Options::default();
    match value {
        None | Some(Value::Nil) => {}
        Some(Value::Integer(millis)) => options.timeout_ms = Some(millis.max(0) as u64),
        Some(Value::Number(millis)) => options.timeout_ms = Some(millis.max(0.0) as u64),
        Some(Value::Table(table)) => {
            let mut bodies = 0;
            for pair in table.pairs::<String, Value>() {
                let (name, value) = pair?;
                match name.as_str() {
                    "timeout_ms" => options.timeout_ms = Some(number(&value, "timeout_ms")?),
                    "ttl" => options.ttl = Some(number(&value, "ttl")?),
                    "headers" => options.headers = string_pairs(&value, "headers")?,
                    "json" => {
                        bodies += 1;
                        let json: serde_json::Value = lua.from_value(value).map_err(|error| {
                            mlua::Error::RuntimeError(format!("http json body: {error}"))
                        })?;
                        let text = serde_json::to_string(&json).map_err(|error| {
                            mlua::Error::RuntimeError(format!("http json body: {error}"))
                        })?;
                        options.body = Body::Json(text);
                    }
                    "form" => {
                        bodies += 1;
                        options.body = Body::Form(string_pairs(&value, "form")?);
                    }
                    "body" => {
                        bodies += 1;
                        let Value::String(text) = value else {
                            return Err(mlua::Error::RuntimeError(format!(
                                "http body must be a string, got {value:?}"
                            )));
                        };
                        options.body = Body::Raw(text.as_bytes().to_vec());
                    }
                    other => {
                        return Err(mlua::Error::RuntimeError(format!(
                            "http options carry no {other:?}"
                        )));
                    }
                }
            }
            if bodies > 1 {
                return Err(mlua::Error::RuntimeError(
                    "http options: json, form and body are mutually exclusive".to_string(),
                ));
            }
            if bodies > 0 && method != "post" {
                return Err(mlua::Error::RuntimeError(
                    "http.get sends no body; use http.post".to_string(),
                ));
            }
            if options.ttl.is_some() && method == "post" {
                return Err(mlua::Error::RuntimeError(
                    "http.post is never cached".to_string(),
                ));
            }
        }
        Some(other) => {
            return Err(mlua::Error::RuntimeError(format!(
                "http options must be a table or a timeout in ms, got {other:?}"
            )));
        }
    }
    Ok(options)
}

fn number(value: &Value, name: &str) -> mlua::Result<u64> {
    match value {
        Value::Integer(number) => Ok((*number).max(0) as u64),
        Value::Number(number) => Ok(number.max(0.0) as u64),
        other => Err(mlua::Error::RuntimeError(format!(
            "http {name} must be a number, got {other:?}"
        ))),
    }
}

fn string_pairs(value: &Value, name: &str) -> mlua::Result<Vec<(String, String)>> {
    let Value::Table(table) = value else {
        return Err(mlua::Error::RuntimeError(format!(
            "http {name} must be a table, got {value:?}"
        )));
    };
    let mut pairs = Vec::new();
    for pair in table.pairs::<String, String>() {
        pairs.push(pair?);
    }
    Ok(pairs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::Path;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    fn lua_with_http(dir: &Path) -> Lua {
        let lua = Lua::new();
        let active: kv::Active = Rc::new(RefCell::new(Some("demo".to_string())));
        let paths: kv::Paths = Rc::new(RefCell::new(
            std::iter::once(("demo".to_string(), dir.join("demo").join("kv.db"))).collect(),
        ));
        let stores: kv::Handle = Rc::new(RefCell::new(kv::Stores::default()));
        lua.globals()
            .set(
                "http",
                lib(&lua, "/test/script.lua", stores, active, paths).unwrap(),
            )
            .unwrap();
        lua
    }

    fn url_of(port: u16, path: &str) -> String {
        format!("http://127.0.0.1:{port}{path}")
    }

    /// The request head and body one client sent, read to the content length.
    fn read_request(stream: &mut TcpStream) -> (String, String) {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut head = String::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                break;
            }
            head.push_str(&line);
        }
        let mut length = 0;
        for line in head.lines() {
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                length = value.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0_u8; length];
        reader.read_exact(&mut body).unwrap();
        (head, String::from_utf8_lossy(&body).into_owned())
    }

    /// One connection on a fresh port; the handler's value comes back on join,
    /// so an assertion inside it fails the test.
    fn serve<T: Send + 'static>(
        handler: impl FnOnce(TcpStream) -> T + Send + 'static,
    ) -> (u16, std::thread::JoinHandle<T>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            handler(stream)
        });
        (port, server)
    }

    #[test]
    fn a_get_carries_params_and_headers_and_answers_status_headers_and_body() {
        let (port, server) = serve(|mut stream| {
            let (head, _) = read_request(&mut stream);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nX-Thing: ok\r\n\r\nok")
                .unwrap();
            head
        });
        let dir = tempfile::tempdir().unwrap();
        let lua = lua_with_http(dir.path());
        let (status, body, thing): (u16, String, String) = lua
            .load(format!(
                r#"
                local res = http.get("{url}", {{
                    q = "rust lang",
                    headers = {{ ["X-Test"] = "yes" }},
                }}, 2000)
                return res.status, res.body, res.headers["x-thing"]
                "#,
                url = url_of(port, "/search"),
            ))
            .eval()
            .unwrap();
        assert_eq!((status, body.as_str(), thing.as_str()), (200, "ok", "ok"));

        let head = server.join().unwrap();
        assert!(head.contains("x-test: yes"), "{head}");
        assert!(head.contains("q=rust%20lang"), "{head}");
    }

    #[test]
    fn a_post_sends_a_json_body_with_its_content_type() {
        let (port, server) = serve(|mut stream| {
            let (head, body) = read_request(&mut stream);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
            (head, body)
        });
        let dir = tempfile::tempdir().unwrap();
        let lua = lua_with_http(dir.path());
        lua.load(format!(
            r#"http.post("{url}", nil, {{ json = {{ name = "demo", n = 3 }} }})"#,
            url = url_of(port, "/post"),
        ))
        .exec()
        .unwrap();

        let (head, body) = server.join().unwrap();
        assert!(
            head.to_lowercase()
                .contains("content-type: application/json"),
            "{head}"
        );
        let sent: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(sent, serde_json::json!({ "name": "demo", "n": 3 }));
    }

    #[test]
    fn a_post_sends_a_form_body() {
        let (port, server) = serve(|mut stream| {
            let (head, body) = read_request(&mut stream);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
            (head, body)
        });
        let dir = tempfile::tempdir().unwrap();
        let lua = lua_with_http(dir.path());
        lua.load(format!(
            r#"http.post("{url}", nil, {{ form = {{ a = "1", b = "two words" }} }})"#,
            url = url_of(port, "/form"),
        ))
        .exec()
        .unwrap();

        let (head, body) = server.join().unwrap();
        assert!(
            head.to_lowercase()
                .contains("application/x-www-form-urlencoded"),
            "{head}"
        );
        let mut fields: Vec<&str> = body.split('&').collect();
        fields.sort_unstable();
        assert_eq!(fields, ["a=1", "b=two+words"]);
    }

    #[test]
    fn a_millisecond_timeout_bounds_the_call() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request(&mut stream);
            std::thread::sleep(Duration::from_secs(2));
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        });
        let dir = tempfile::tempdir().unwrap();
        let lua = lua_with_http(dir.path());
        let started = Instant::now();
        let (ok, raised): (bool, bool) = lua
            .load(format!(
                r#"
                local ok, err = pcall(http.get, "{url}", nil, 200)
                return ok, err ~= nil
                "#,
                url = url_of(port, "/slow"),
            ))
            .eval()
            .unwrap();
        assert!(!ok, "the call raised");
        assert!(raised);
        assert!(
            started.elapsed() < Duration::from_millis(1_500),
            "the timeout fired before the server answered"
        );
    }

    #[test]
    fn a_ttl_serves_the_second_call_from_the_store() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&seen);
        std::thread::spawn(move || {
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
    fn a_non_2xx_answers_with_its_status() {
        let (port, _server) = serve(|mut stream| {
            let _ = read_request(&mut stream);
            stream
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 4\r\n\r\nnope")
                .unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let lua = lua_with_http(dir.path());
        let (status, body): (u16, String) = lua
            .load(format!(
                r#"
                local res = http.get("{url}", nil, 2000)
                return res.status, res.body
                "#,
                url = url_of(port, "/missing"),
            ))
            .eval()
            .unwrap();
        assert_eq!((status, body.as_str()), (404, "nope"));
    }

    #[test]
    fn a_non_utf8_body_crosses_as_bytes() {
        let (port, _server) = serve(|mut stream| {
            let _ = read_request(&mut stream);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n\x80\x81")
                .unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let lua = lua_with_http(dir.path());
        let (len, first, second): (i64, i64, i64) = lua
            .load(format!(
                r#"
                local res = http.get("{url}", nil, 2000)
                return #res.body, res.body:byte(1), res.body:byte(2)
                "#,
                url = url_of(port, "/bytes"),
            ))
            .eval()
            .unwrap();
        assert_eq!((len, first, second), (2, 128, 129));
    }

    #[test]
    fn a_gzip_response_arrives_decompressed() {
        // gzip of "ok", so this test needs no compression stack of its own.
        const GZIPPED: &[u8] = &[
            0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xff, 0xcb, 0xcf, 0x06, 0x00,
            0x47, 0xdd, 0xdc, 0x79, 0x02, 0x00, 0x00, 0x00,
        ];
        let (port, _server) = serve(|mut stream| {
            let _ = read_request(&mut stream);
            let mut response =
                b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: ".to_vec();
            response.extend_from_slice(GZIPPED.len().to_string().as_bytes());
            response.extend_from_slice(b"\r\n\r\n");
            response.extend_from_slice(GZIPPED);
            stream.write_all(&response).unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let lua = lua_with_http(dir.path());
        let body: String = lua
            .load(format!(
                r#"return http.get("{url}", nil, 2000).body"#,
                url = url_of(port, "/gzip"),
            ))
            .eval()
            .unwrap();
        assert_eq!(body, "ok");
    }

    #[test]
    fn bad_options_raise_before_any_request() {
        let dir = tempfile::tempdir().unwrap();
        let lua = lua_with_http(dir.path());
        let (body_on_get, unknown_key, ttl_on_post): (bool, bool, bool) = lua
            .load(
                r#"
                local url = "http://127.0.0.1:9/x"
                local a = pcall(http.get, url, nil, { json = {} })
                local b = pcall(http.get, url, nil, { timeout = 5 })
                local c = pcall(http.post, url, nil, { ttl = 5 })
                return a, b, c
                "#,
            )
            .eval()
            .unwrap();
        assert!(!body_on_get, "a get takes no body option");
        assert!(!unknown_key, "an unknown option is refused");
        assert!(!ttl_on_post, "a post is never cached");
    }

    #[test]
    fn timeouts_are_clamped_inside_the_host_ceiling() {
        let ceiling = crate::provider::external::HOST_TIMEOUT - Duration::from_millis(500);
        assert_eq!(timeout_for(None), Duration::from_millis(2_000));
        assert_eq!(timeout_for(Some(50)), Duration::from_millis(100));
        assert_eq!(timeout_for(Some(3_000)), Duration::from_millis(3_000));
        assert_eq!(timeout_for(Some(60_000)), ceiling);
    }
}
