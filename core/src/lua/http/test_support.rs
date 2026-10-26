use std::cell::RefCell;
use std::io::{BufRead, BufReader, Read};
use std::iter;
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::rc::Rc;
use std::thread;

use mlua::Lua;

use super::lib;
use crate::lua::kv;

pub(super) fn lua_with_http(dir: &Path) -> Lua {
    let lua = Lua::new();
    let active: kv::Active = Rc::new(RefCell::new(Some("demo".to_string())));
    let paths: kv::Paths = Rc::new(RefCell::new(
        iter::once(("demo".to_string(), dir.join("demo").join("kv.db"))).collect(),
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

pub(super) fn url_of(port: u16, path: &str) -> String {
    format!("http://127.0.0.1:{port}{path}")
}

/// The request head and body one client sent, read to the content length.
pub(super) fn read_request(stream: &mut TcpStream) -> (String, String) {
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

/// One connection on a fresh port; the handler's value comes back on join, so
/// an assertion inside it fails the test.
pub(super) fn serve<T: Send + 'static>(
    handler: impl FnOnce(TcpStream) -> T + Send + 'static,
) -> (u16, thread::JoinHandle<T>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        handler(stream)
    });
    (port, server)
}
