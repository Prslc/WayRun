# Lua plugins

A plugin can be one Lua script, hosted by the launcher binary itself:
`wayrun --lua-host <script.lua>` runs the script and answers the same JSON-RPC
surface an [external host](plugins.md#external-hosts) speaks. The core treats
Lua and Python plugins alike — the Lua one just starts faster and needs no
interpreter installed.

## Writing a script

A script returns a list of plugin tables; each table is one plugin:

```lua
return {
  {
    id = "demo",                         -- must match the plugins.toml entry
    name = "Demo",                       -- shown in the help cards
    icon = wayrun.icon("builtin:globe"), -- an absolute path, or nil
    description = "Say hello",           -- optional, shown in the help cards
    search = function(text)              -- rows for a routed query, in order
      if text == "" then return {} end
      return {
        { title = "hello " .. text, summary = "from demo.lua" },
      }
    end,
    top = function() end,                -- optional: the empty-query view
    forget = function(action) end,       -- optional: claim a row for removal
  },
}
```

Rows are the [result items](jsonrpc.md#result-items) of the wire protocol:
`title`, `summary`, `on_click` (an action), `icon`, `ephemeral`, `actions`,
`badge`. No builder is needed — a table in the wire shape is enough, for
example `on_click = { type = "open", uri = "https://example.com" }`.

## The `wayrun` table

| Call | Does |
| --- | --- |
| `wayrun.home()` | `$HOME`, or nil |
| `wayrun.cache_dir()` | the launcher's cache directory, or nil |
| `wayrun.icon(spec)` | resolves `builtin:…`, a theme name or `papirus:…` to an absolute path; nil on a miss |
| `wayrun.urlencode(text)` | percent-encodes for use in a URL |
| `wayrun.t(key, args)` | a UI string from the launcher's own tables, with `%{name}` filled from `args` |
| `wayrun.log(message)` | writes to the launcher's journal under the script's name |
| `wayrun.web_search_engine()` | the configured search engine, e.g. `"google"` |
| `wayrun.time()` | Unix seconds, for signatures and cache TTLs |
| `wayrun.env(name)` | the launcher process's variable `name`, or nil |
| `wayrun.script_dir()` | the script's own directory, for an icon it ships |
| `wayrun.plugin_dir(id)` | the directory a plugin may read from, `~/.config/wayrun/plugins/<id>` |
| `wayrun.fs.read(name)` | a file under those directories; a relative name, nil when absent, an error when the name escapes |
| `wayrun.toml.decode(text)` | TOML in, a table out |
| `wayrun.json.decode(text)` / `wayrun.json.encode(value)` | JSON in and out |
| `wayrun.fs.list(dir)` | entry names under `dir`, or nil |
| `wayrun.fs.stat(path)` | `{ mtime_ns, size }`, or nil |
| `wayrun.http.get(url, params?, timeout_ms?)` | blocking GET; `params` adds query values |
| `wayrun.sqlite.snapshot(path)` | an immutable copy of a SQLite file, opened and returned as a handle |
| `wayrun.sqlite.query(handle, sql, params?)` | rows as tables; a NULL column reads as absent |

## The sandbox

`os`, `io`, `package`, `load` and `print` are not reachable: a script cannot
run programs, and the only files it can read are its own, through `fs.read`,
confined to `~/.config/wayrun/plugins/<id>/`; the launcher's environment,
through `wayrun.env`, is the one other part of its process a script can touch.
A call that raises yields no rows and reports to the journal, so `wayrun.log`
and a `pcall` around risky work are the debugging tools.

## Limits

- A Lua plugin needs a non-empty keyword: it answers its own routed queries.
  The default chain (keyword `""`) is reserved for the built-ins.
- Rows keep the order the script returns; there is no relevance channel.
- Icons must be absolute paths; `wayrun.icon` is how to get one.

## Registering

Like any host: a single executable token, so the script needs a shebang and the
exec bit — or the `script` key for the ones shipped in the binary.

```toml
[[plugins]]
id = "demo"
keyword = "dm"
command = "/home/you/.config/wayrun/demo.lua"
resident = true   # keep one host process warm between calls
```

The first line of the script must be
`#!/usr/bin/env -S wayrun --lua-host`. `firefox.lua` and `web.lua` under
`core/assets/lua/` are complete worked examples, read from `places.sqlite` and
the web respectively.
