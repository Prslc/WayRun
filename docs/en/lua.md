# Lua plugins

A plugin can also be one Lua script — no framework, no Python install. The
WayRun binary hosts it itself (`wayrun --lua-host`), answering the same
JSON-RPC surface a Python host does, so it starts in milliseconds — and
everything the launcher provides is reachable through one `wayrun` table.

## Writing a script

A script returns a list of plugin tables; each table is one plugin:

```lua
#!/usr/bin/env -S wayrun --lua-host
return {
  {
    id = "my-plugin",                    -- must match the plugins.toml id
    name = "My Plugin",
    -- icon = wayrun.script_dir() .. "/icon.svg", -- an icon file you ship beside the script
    description = "Short description",
    api = 1,                             -- the plugin contract it was written against
    env = { "MY_TOKEN" },                -- the env names it may read
    search = function(text)              -- rows for a routed query, in order
      return {
        { title = "You searched: " .. text,
          on_click = { type = "copy", text = text } },
      }
    end,
    top = function() end,                -- optional: the empty-query view
  },
}
```

A row is a plain table in the shape [jsonrpc.md](jsonrpc.md#result-items)
documents — `title`, `summary`, `on_click`, `icon` and `actions` — so no builder
is needed: `on_click = { type = "open", uri = "https://example.com" }`.

## The plugin table

`id` is required and must match the `plugins.toml` entry, and `name`, `icon` and
`description` are the identity the launcher shows, in the shape
[jsonrpc.md](jsonrpc.md#external-host--core-identity-discovery) gives them, with
the icon rule in [plugins.md](plugins.md#external-hosts). `search` answers a
routed query and `top` the empty one. `api` states which plugin contract the
script was written against — the rule for that number is in
[plugins.md](plugins.md) — and `wayrun.api` reads the one the running launcher
speaks, so one script can span two.

`env` lists the exact environment variable names the plugin may read, no
patterns. An undeclared name raises; a declared-but-unset one is nil. Reads work
inside `search` and `top` and not while the script loads — for the paths a
script would otherwise take from the environment, use `wayrun.home()` and
`wayrun.cache_dir()`.

`read` lists the areas a plugin may read — absolute paths or `~/…`, exact, no
patterns. Symlinks are resolved on every read, so a link inside an area cannot
lead out of it; `fs.list`, `fs.stat` and `sqlite.snapshot` raise for anything
outside those areas, while the plugin's own directory and the script's
directory are always inside.

## Actions

`on_click` and every entry of a row's `actions` are commands: plain tables
tagged by `type`, in the shapes [jsonrpc.md](jsonrpc.md#actions) lists. A row's
`actions` fill the `Shift+Enter` panel, and what the launcher puts around them is
in [plugins.md](plugins.md#result-actions).

```lua
actions = {
  {
    title = "Copy URL",
    id = "copy_url",                    -- lets the user make it the row's default
    action = { type = "copy", text = uri },
    icon = wayrun.script_dir() .. "/copy.svg", -- an icon file you ship, like every icon
  },
},
```

A row with no `on_click` is display-only; it can still carry `actions`.

## The `wayrun` table

| Call | Does |
| --- | --- |
| `wayrun.api` | the plugin contract the running launcher speaks, for a script that spans two |
| `wayrun.home()` | `$HOME`, or nil |
| `wayrun.cache_dir()` | the launcher's cache directory, or nil |
| `wayrun.icon(name)` | resolves a theme icon name or `papirus:…` spec to an absolute path (an absolute path passes through); nil on a miss or a `builtin:` glyph |
| `wayrun.urlencode(text)` | percent-encodes for use in a URL |
| `wayrun.log(message)` | writes to the launcher's journal under the script's name |
| `wayrun.time()` | Unix seconds, for signatures and cache TTLs |
| `wayrun.env(name)` | a variable the plugin declares in `env`, or nil when unset; an undeclared name raises |
| `wayrun.which(name)` | an executable's path on the launcher's `$PATH`, or nil; the same PATH a `run` row's command will see |
| `wayrun.script_dir()` | the script's own directory, for an icon it ships |
| `wayrun.plugin_dir(id)` | the directory a plugin may read from, `~/.config/wayrun/plugins/<id>` |
| `wayrun.fs.read(name)` | a file under those directories; a relative name, nil when absent, an error when the name escapes |
| `wayrun.toml.decode(text)` | TOML in, a table out |
| `wayrun.json.decode(text)` / `wayrun.json.encode(value)` | JSON in and out |
| `wayrun.fs.list(dir)` | sorted entry names under `dir` inside a declared area, or nil |
| `wayrun.fs.stat(path)` | `{ mtime_ns, size }` for a path inside a declared area, or nil |
| `wayrun.http.get(url, params?, options?)` | blocking GET answering `{status, headers, body}`; a transport error raises; `params` adds query values and takes a reserved `headers` table; `options` is a table with `timeout_ms`, `headers` and `ttl`, or a bare timeout in ms |
| `wayrun.http.post(url, params?, options?)` | the same, sending one body: `json = value`, `form = {…}` or `body = "…"`; never cached |
| `wayrun.crypto.sha256(text)` / `wayrun.crypto.md5(text)` | lowercase hex digest |
| `wayrun.crypto.hmac_sha256(key, text)` | lowercase hex HMAC |
| `wayrun.crypto.base64_encode(data)` / `wayrun.crypto.base64_decode(text)` | base64 both ways over raw bytes; decode answers nil on garbage |
| `wayrun.sqlite.snapshot(path)` | an immutable copy of a SQLite file inside a declared area, opened and returned as a handle; one file answers one handle, following its newest copy |
| `wayrun.sqlite.query(handle, sql, params?)` | rows as tables; a NULL column reads as absent; a handle's source file must lie inside the calling plugin's areas |
| `wayrun.kv.get(key)` | a stored string, or nil when absent or expired |
| `wayrun.kv.set(key, value, ttl_secs?)` | stores a string under `key`; with a ttl it expires; the store is a sqlite database in the plugin's own directory |
| `wayrun.kv.delete(key)` | drops a key |
| `wayrun.kv.keys(prefix?)` / `wayrun.kv.pairs(prefix?)` | the unexpired keys, or `{key, value}` records, under a prefix, ordered by key |
| `wayrun.fuzzy.match(query, candidates)` | `{index, kind}` hits, best first, over an array of strings; the kinds are the launcher's own (exact, prefix, word, substring, loose) |

`http.get` with a `ttl` answers a repeated call from the plugin's own store
instead of the network — a 2xx text reply only, which is what makes a
per-keystroke query affordable. Replies carry the body as raw bytes and
lowercase response header names, and a timeout (two seconds by default) is
capped inside the host call's own ceiling, so a slow endpoint raises an error
for `pcall` rather than dying as a killed host.

## The sandbox

`os`, `io`, `package`, `load` and `print` are not reachable: a script cannot
run programs, and the only write is `kv`, whose keys are not paths and whose
database lives in the plugin's own directory. Reading is wider: `fs.read` is
the scoped one (relative names only, confined to
`~/.config/wayrun/plugins/<id>/`), while `fs.list`, `fs.stat` and
`sqlite.snapshot` reach only the areas a plugin declares for itself
(`firefox.lua` declares `~/.mozilla/firefox` for `places.sqlite`),
`wayrun.env` reads only the names the calling plugin declares, and `http`
reaches the network. The sandbox bounds what a script can do; what it can read
is what the plugin's `env` and `read` declare. A call that raises yields no rows
and reports to the journal, so `wayrun.log` and a `pcall` around risky work are
the debugging tools.

## Limits

- A Lua plugin needs a non-empty keyword: it answers its own routed queries.
  The default chain (keyword `""`) is reserved for the built-ins.
- Rows keep the order the script returns; there is no relevance channel.
- Icons follow the rule in [plugins.md](plugins.md#external-hosts): a file the
  script ships, or a theme icon `wayrun.icon(name)` resolved.

## Registering

Copy `template.lua`, keep the shebang and the exec bit (`chmod +x`), and
register it like any host — a single executable token, with the fields in
[plugins.md](plugins.md#entry-fields) and the contract in
[jsonrpc.md](jsonrpc.md). The first line of the script must be
`#!/usr/bin/env -S wayrun --lua-host`.

[firefox.lua](https://github.com/Prslc/WayRun-Plugins/blob/main/example/lua/firefox.lua)
and [web.lua](https://github.com/Prslc/WayRun-Plugins/blob/main/example/lua/web.lua)
are complete worked examples, reading `places.sqlite` and the web respectively.
