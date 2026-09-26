//! The shipped Lua scripts against the host contract: a fixture places.sqlite
//! goes in, the row shapes the core depends on come out.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn script(name: &str) -> String {
    format!("{}/../core/assets/lua/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// One fresh host process answers every request, then exits on stdin EOF.
fn ask(home: &Path, script: &str, requests: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_wayrun"))
        .arg("--lua-host")
        .arg(script)
        .env("HOME", home)
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawning the lua host");
    let stdin = child.stdin.as_mut().expect("piped stdin");
    for request in requests {
        writeln!(stdin, "{request}").expect("writing a request");
    }
    let output = child.wait_with_output().expect("reaping the lua host");
    assert!(
        output.status.success(),
        "host exited with {}",
        output.status
    );
    String::from_utf8(output.stdout)
        .expect("utf-8 replies")
        .lines()
        .map(|line| serde_json::from_str(line).expect("one JSON reply per line"))
        .collect()
}

fn fixture(home: &Path) -> PathBuf {
    let db = home.join(".mozilla/firefox/prof/places.sqlite");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE moz_places (id INTEGER PRIMARY KEY, title TEXT, url TEXT, last_visit_date INTEGER);
             CREATE TABLE moz_bookmarks (id INTEGER PRIMARY KEY, fk INTEGER, title TEXT, dateAdded INTEGER);
             INSERT INTO moz_places (title, url, last_visit_date) VALUES
               ('Rust Programming Language', 'https://www.rust-lang.org/', 1700000000000000),
               ('GitHub', 'https://github.com/', 1700000001000000),
               (NULL, 'https://example.com/untitled', 1700000002000000);
             INSERT INTO moz_bookmarks (fk, title, dateAdded) VALUES
               (1, 'Rust Lang', 1000), (2, 'My GitHub', 2000);",
        )
        .unwrap();
    db
}

fn search(plugin: &str, text: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "search",
        "params": { "plugin": plugin, "text": text },
        "id": 1,
    })
}

#[test]
fn firefox_scripts_answer_the_host_contract() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let replies = ask(
        dir.path(),
        &script("firefox.lua"),
        &[
            serde_json::json!({"jsonrpc": "2.0", "method": "list_plugins", "id": 1}),
            search("firefox-bookmarks", "rust"),
            search("firefox-history", "hub"),
            search("firefox-history", "example"),
            search("firefox-bookmarks", "zzz"),
            search("firefox-bookmarks", ""),
        ],
    );

    let identities = replies[0]["result"].as_array().unwrap();
    assert_eq!(identities.len(), 2, "one script serves both plugins");
    assert_eq!(identities[0]["id"], "firefox-bookmarks");
    assert_eq!(identities[1]["id"], "firefox-history");
    for identity in identities {
        assert!(!identity["name"].as_str().unwrap().is_empty());
        assert!(
            identity["icon"].as_str().unwrap().starts_with('/'),
            "a host identity icon is an absolute path"
        );
    }

    let bookmarks = replies[1]["result"].as_array().unwrap();
    assert_eq!(
        bookmarks.len(),
        1,
        "the bookmark's own title wins the match"
    );
    let row = &bookmarks[0];
    assert_eq!(row["title"], "Rust Lang", "the user-edited bookmark title");
    assert_eq!(row["summary"], "https://www.rust-lang.org/");
    assert_eq!(row["on_click"]["type"], "open");
    assert_eq!(row["on_click"]["uri"], "https://www.rust-lang.org/");
    assert_eq!(row["ephemeral"], false);
    assert!(row["icon"].as_str().unwrap().starts_with('/'));
    assert_eq!(row["actions"][0]["id"], "copy_url");
    assert_eq!(row["actions"][0]["action"]["type"], "execute");
    assert_eq!(row["actions"][0]["action"]["command"]["type"], "copy");

    let history = replies[2]["result"].as_array().unwrap();
    assert_eq!(history[0]["title"], "GitHub");

    let untitled = replies[3]["result"].as_array().unwrap();
    assert_eq!(
        untitled[0]["title"], "[no title]",
        "a NULL title has a stand-in"
    );

    assert_eq!(
        replies[4]["result"],
        serde_json::json!([]),
        "no match, no rows"
    );
    assert_eq!(
        replies[5]["result"],
        serde_json::json!([]),
        "an empty query never queries"
    );
}

#[test]
fn the_web_script_names_its_engine() {
    let dir = tempfile::tempdir().unwrap();
    let replies = ask(
        dir.path(),
        &script("web.lua"),
        &[serde_json::json!({"jsonrpc": "2.0", "method": "list_plugins", "id": 1})],
    );
    let identities = replies[0]["result"].as_array().unwrap();
    assert_eq!(identities.len(), 1);
    assert_eq!(identities[0]["id"], "web-search");
    assert!(!identities[0]["name"].as_str().unwrap().is_empty());
    assert!(identities[0]["icon"].as_str().unwrap().starts_with('/'));
}
