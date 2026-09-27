use std::time::Duration;

use mlua::{Lua, Table, Value};

/// `wayrun.http`: the sandbox's one network exit. One agent per host keeps the
/// connection pool; every request carries its own deadline.
pub(super) fn lib(lua: &Lua) -> mlua::Result<Table> {
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    let config = ureq::config::Config::builder()
        .http_status_as_error(false)
        .proxy(ureq::Proxy::try_from_env())
        .tls_config(tls)
        .build();
    let agent = ureq::Agent::new_with_config(config);

    let http = lua.create_table()?;
    http.set(
        "get",
        lua.create_function(
            move |lua, (url, params, timeout_ms): (String, Option<Table>, Option<u64>)| {
                let seconds = (timeout_ms.unwrap_or(5_000) / 1_000).max(1);
                let mut request = agent.get(&url);
                if let Some(params) = params {
                    for pair in params.pairs::<String, Value>() {
                        let (name, value) = pair?;
                        if name == "headers" {
                            let Value::Table(headers) = value else {
                                return Err(mlua::Error::RuntimeError(format!(
                                    "http headers must be a table, got {value:?}"
                                )));
                            };
                            for pair in headers.pairs::<String, String>() {
                                let (key, value) = pair?;
                                request = request.header(key, value);
                            }
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
                        request = request.query(name, value);
                    }
                }
                let mut response = request
                    .config()
                    .timeout_global(Some(Duration::from_secs(seconds)))
                    .build()
                    .call()
                    .map_err(|error| {
                        mlua::Error::RuntimeError(format!("http.get {url}: {error}"))
                    })?;
                let status = response.status().as_u16();
                let body = response.body_mut().read_to_string().map_err(|error| {
                    mlua::Error::RuntimeError(format!("http.get {url}: {error}"))
                })?;
                let reply = lua.create_table()?;
                reply.set("status", status)?;
                reply.set("body", body)?;
                Ok(reply)
            },
        )?,
    )?;
    Ok(http)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_get_sends_its_headers_and_answers_status_and_body() {
        use std::io::{BufRead, BufReader, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
                head.push_str(&line);
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .unwrap();
            head
        });

        let lua = Lua::new();
        lua.globals().set("http", lib(&lua).unwrap()).unwrap();
        lua.globals()
            .set("url", format!("http://127.0.0.1:{port}/search"))
            .unwrap();
        let (status, body): (u16, String) = lua
            .load(
                r#"
                local res = http.get(url, {
                    q = "rust lang",
                    headers = { ["X-Test"] = "yes" },
                }, 2000)
                return res.status, res.body
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!((status, body.as_str()), (200, "ok"));

        let head = server.join().unwrap();
        assert!(head.contains("x-test: yes"), "{head}");
        assert!(head.contains("q=rust%20lang"), "{head}");
    }
}
