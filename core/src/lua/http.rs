use std::iter;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use mlua::{Lua, Table, Value};

use self::cache::cache_key;
use self::options::{Options, parse_options, with_params};
use self::transport::{agent, perform};
use super::kv;

mod cache;
mod options;
mod transport;

#[cfg(test)]
mod test_support;

/// The most `get_many` requests in flight at once.
const FANOUT: usize = 4;

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
    let many = client.clone();
    http.set(
        "get_many",
        lua.create_function(move |lua, requests: Table| many.get_many(lua, requests))?,
    )?;
    let cached = client.clone();
    http.set(
        "cached",
        lua.create_function(
            move |lua, (url, params, options): (String, Option<Table>, Option<Value>)| {
                cached.cached_reply(lua, url, params, options)
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
        let key = cache_key(&url, &options.headers);
        if ttl.is_some()
            && let Some(cached) = self.cached(&key)
        {
            return reply(lua, &cached);
        }
        let response = perform(&self.agent, method, &url, &options, post)
            .map_err(mlua::Error::RuntimeError)?;
        if let Some(ttl) = ttl
            && (200..300).contains(&response.status)
        {
            self.store(&key, &response, ttl);
        }
        reply(lua, &response)
    }

    /// Several GETs in flight at once, answered in input order; a transport
    /// failure holds its slot as `{ error = "…" }` instead of failing the batch.
    fn get_many(&self, lua: &Lua, requests: Table) -> mlua::Result<Table> {
        struct Item {
            url: String,
            options: Options,
            ttl: Option<u64>,
            key: String,
        }

        let mut items = Vec::new();
        for request in requests.sequence_values::<Table>() {
            let request = request?;
            let url: String = request.get("url")?;
            let params: Option<Table> = request.get("params")?;
            let options: Option<Value> = request.get("options")?;
            let mut options = parse_options(lua, options, "get")?;
            let (url, mut headers) = with_params(url, params)?;
            headers.append(&mut options.headers);
            options.headers = headers;
            let ttl = options.ttl.filter(|ttl| *ttl > 0);
            let key = cache_key(&url, &options.headers);
            items.push(Item {
                url,
                options,
                ttl,
                key,
            });
        }

        let mut outcomes: Vec<Option<Result<Response, String>>> =
            iter::repeat_with(|| None).take(items.len()).collect();
        let mut misses = Vec::new();
        for (at, item) in items.iter().enumerate() {
            if item.ttl.is_some()
                && let Some(cached) = self.cached(&item.key)
            {
                outcomes[at] = Some(Ok(cached));
            } else {
                misses.push(at);
            }
        }

        if !misses.is_empty() {
            let next = AtomicUsize::new(0);
            let results: Mutex<Vec<Option<Result<Response, String>>>> =
                Mutex::new(iter::repeat_with(|| None).take(items.len()).collect());
            let agent = &self.agent;
            thread::scope(|scope| {
                for _ in 0..FANOUT.min(misses.len()) {
                    scope.spawn(|| {
                        loop {
                            let at = next.fetch_add(1, Ordering::Relaxed);
                            let Some(&slot) = misses.get(at) else { break };
                            let item = &items[slot];
                            let outcome = perform(agent, "get", &item.url, &item.options, false);
                            results.lock().expect("http fan-out")[slot] = Some(outcome);
                        }
                    });
                }
            });
            outcomes = results.into_inner().expect("http fan-out");
        }

        let replies = lua.create_table_with_capacity(items.len(), 0)?;
        for (at, outcome) in outcomes.into_iter().enumerate() {
            let value = match outcome.expect("every request has an outcome") {
                Ok(response) => {
                    if let Some(ttl) = items[at].ttl
                        && (200..300).contains(&response.status)
                    {
                        self.store(&items[at].key, &response, ttl);
                    }
                    reply(lua, &response)?
                }
                Err(error) => {
                    let failure = lua.create_table()?;
                    failure.set("error", error)?;
                    failure
                }
            };
            replies.set(at + 1, value)?;
        }
        Ok(replies)
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

#[cfg(test)]
mod tests {
    use crate::lua::http::test_support::{lua_with_http, read_request, serve, url_of};
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread;
    use std::time::{Duration, Instant};

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
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request(&mut stream);
            thread::sleep(Duration::from_secs(2));
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
    fn get_many_answers_in_order_and_keeps_a_failed_slot() {
        let (port_a, server_a) = serve(|mut stream| {
            let (head, _) = read_request(&mut stream);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\na")
                .unwrap();
            head
        });
        let (port_b, server_b) = serve(|mut stream| {
            let (head, _) = read_request(&mut stream);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\nb")
                .unwrap();
            head
        });
        let dir = tempfile::tempdir().unwrap();
        let lua = lua_with_http(dir.path());
        let (first, last, failed): (String, String, bool) = lua
            .load(format!(
                r#"
                local replies = http.get_many({{
                    {{ url = "{a}" }},
                    {{ url = "http://127.0.0.1:1/x" }},
                    {{ url = "{b}" }},
                }})
                return replies[1].body, replies[3].body, replies[2].error ~= nil
                "#,
                a = url_of(port_a, "/a"),
                b = url_of(port_b, "/b"),
            ))
            .eval()
            .unwrap();
        assert_eq!((first.as_str(), last.as_str(), failed), ("a", "b", true));
        assert!(server_a.join().unwrap().contains("/a"));
        assert!(server_b.join().unwrap().contains("/b"));
    }
}
