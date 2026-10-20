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
    search = function(text, emit)        -- rows for a routed query, in order
      return {
        { title = "You searched: " .. text,
          on_click = { type = "copy", text = text } },
      }
    end,
    top = function() end,                -- optional: the empty-query view
    command = function(action, payload) end, -- optional: a row's plugin action
  },
}
```

A row is a plain table in the shape [jsonrpc.md](jsonrpc.md#result-items)
documents — `title`, `summary`, `on_click`, `icon`, `icon_shape`, `preview` and
`actions` — so no builder is needed:
`on_click = { type = "open", uri = "https://example.com" }`.
A `preview = { type = "image", path = "/absolute/file.png" }` (or an `https://`
URL the core caches) opens the split view beside the list for the selected row;
PNG, SVG, JPEG, WebP, GIF and BMP decode in this version. `preview = { type = "text", text = "…" }` puts a
wrapped plain-text excerpt in the same column.

A streaming search also receives `emit`: when the core supports streaming it
passes a function, and each `emit(rows)` shows those rows at once while the
search keeps running. The final return is still the authoritative payload. An
older core passes `nil`, so guard it with `if emit then … end`; that core never
sends the capability, and the plugin answers once as before.

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

`exec` lists the exact program names (on `PATH`, no slashes) a plugin may run
with `wayrun.exec`. The child starts in `$HOME` and keeps the session's `PATH`,
`HOME`, XDG, D-Bus and Wayland variables plus the plugin's declared `env` names;
nothing else from the launcher's environment is inherited.

`command` receives the `action` and `payload` of a row's
`{ type = "plugin", … }` action (see [Actions](#actions)). It runs in the
plugin's own scope, so `fs.write`, `env` and `exec` work as they do in a search,
and the launcher is already dismissing by then: a failure reaches the journal
rather than the list, and the return value is ignored.

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

A `{ type = "plugin", … }` action commits something only the plugin can do. The
launcher hands it back to the script's `command` handler instead of running it:

```lua
on_click = { type = "plugin", plugin = "my-plugin", action = "new",
             payload = { title = "Shopping list" } },
-- command = function(action, payload) ... end writes the file, no shell needed
```

## The `wayrun` table

| Call | Does |
| --- | --- |
| `wayrun.api` | the plugin contract the running launcher speaks, for a script that spans two |
| `wayrun.home()` | `$HOME`, or nil |
| `wayrun.cache_dir()` | the launcher's cache directory, or nil |
| `wayrun.icon(name)` | resolves a theme icon name or `papirus:…` spec to an absolute path (an absolute path passes through); nil on a miss or a `builtin:` glyph |
| `wayrun.urlencode(text)` | percent-encodes for use in a URL |
| `wayrun.file_uri(path)` | the `file://` URI for an absolute path, encoded like a built-in row's; nil for a relative one |
| `wayrun.log(message)` | writes to the launcher's journal under the script's and calling plugin's name |
| `wayrun.time()` | Unix seconds, for signatures and cache TTLs |
| `wayrun.locale()` | the launcher's resolved locale, the stem its own tables use (`en`, `zh_cn`, …) |
| `wayrun.env(name)` | a variable the plugin declares in `env`, or nil when unset; an undeclared name raises |
| `wayrun.which(name)` | an executable's path on the launcher's `$PATH`, or nil; the same PATH a `run` row's command will see |
| `wayrun.exec(program, args?)` | runs a declared `PATH` program with argv (no shell) in `$HOME` under a 1.5s deadline; answers `{code, stdout, stderr}`, capped at 64 KiB/8 KiB; an undeclared program or a timeout raises |
| `wayrun.script_dir()` | the script's own directory, for an icon it ships |
| `wayrun.import(name)` | loads `<script_dir>/<name>.lua` in the same sandbox and caches its value; a nil return becomes true; a circular or escaping import raises |
| `wayrun.plugin_dir(id)` | the directory a plugin may read from, `$XDG_CONFIG_HOME/wayrun/plugins/<id>` |
| `wayrun.plugin_cache_dir(id)` | the writable cache for a plugin, `$XDG_CACHE_HOME/wayrun/plugins/<id>` |
| `wayrun.fs.read(name)` | a file: a relative name under the plugin directory, or an absolute/`~/` path in a declared area, the plugin's directory, its cache, or the script's directory; nil when absent, an error when it escapes |
| `wayrun.toml.decode(text)` | TOML in, a table out |
| `wayrun.json.decode(text)` / `wayrun.json.encode(value)` | JSON in and out |
| `wayrun.fs.list(dir)` | sorted entry names under a directory in those places, or nil |
| `wayrun.fs.stat(path)` | `{ mtime_ns, size }` for a path in those places, or nil |
| `wayrun.fs.write(path, data)` | writes a string into the plugin directory or its cache (parents created); true on success; `kv.db` and its siblings are reserved |
| `wayrun.fs.delete(path)` | removes a file in the plugin directory or its cache; true when it is already absent |
| `wayrun.http.get(url, params?, options?)` | blocking GET answering `{status, headers, body}`; a transport error raises; `params` adds query values and takes a reserved `headers` table; `options` is a table with `timeout_ms`, `headers` and `ttl`, or a bare timeout in ms |
| `wayrun.http.get_many(requests)` | concurrent GETs: `requests` is an array of `{url, params?, options?}`, answered in order; a transport failure holds its slot as `{error = "…"}` |
| `wayrun.http.cached(url, params?, options?)` | the stored reply without a network call, or nil; `stale` is true when its `ttl` has passed |
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

`http.get_many` runs up to four requests at once and checks each one's `ttl`
cache first; replies keep the input order, and a transport failure is
`{error = "…"}` in its slot instead of raising for the whole batch. The batch
shares the host call's ceiling, so keep the per-request timeouts within it.

`http.cached` answers from the plugin's store alone, expired entries included,
and marks an expired one with `stale = true`. That is what `emit` is for: show
the stale rows first, then call `http.get` and return the fresh ones.

## The sandbox

`os`, `io`, `package` and `load` are not reachable: a script cannot
run programs. Reading is scoped by path: a relative name resolves under the
plugin's own directory (`$XDG_CONFIG_HOME/wayrun/plugins/<id>/`), while an
absolute or `~/` path must lie in a declared read area, the plugin's directory,
its cache, or the script's directory; `sqlite.snapshot` uses the same rule. Writing is
narrower: `fs.write` stays inside the plugin's own directory or its cache
(parents are created); relative names land in the config directory, where
`kv.db` and its `-wal`/`-shm`/`-journal` siblings stay reserved. `fs.delete`
removes files from either place. `wayrun.env` reads only the names the calling
plugin declares, and `http` reaches the network. `print` and `wayrun.log` write to the
launcher's journal, never to stdout (stdout is the JSON-RPC wire), and both are
bounded per call. The sandbox bounds what a script can do; what it can read is
what the plugin's `env` and `read` declare. A `search` call that raises is
shown as one error row carrying the first line of the message; a `top` failure
keeps the identity card. `print`, `wayrun.log` and a `pcall` around risky work
stay the debugging tools.

## Debugging

Drive a script without the launcher and read one reply:

```sh
printf '{"jsonrpc":"2.0","method":"search","params":{"plugin":"<id>","text":"…"},"id":1}\n' \
  | wayrun --lua-host plugin.lua
```

A missing `env` or `read` declaration is named in the error, so add it to the
plugin table instead of probing for it.

`wayrun plugin check <script.lua>` loads the script the way the host does and
reports what would keep it from working: the shebang and the exec bit, the
plugin tables it returns, unknown keys, an `api` newer than the launcher's, a
missing `search`/`top` or identity icon, and whether `plugins.toml` registers
the ids it declares. Problems fail the command; the rest are warnings.

## Modules

`wayrun.import("lib/util")` loads `<script_dir>/lib/util.lua` (the `.lua` is
optional) in the same sandbox environment and returns its value; later imports
of the same canonical path return the cached value. A nil return is cached as
`true`, as `require` does. Names are relative, may not climb out with `..`, and
a symlink that resolves outside the script's directory is refused. A circular
import raises rather than recursing.

A resident host watches every file the script imported too: an edit to one
reloads the sandbox before the next request, and a failed reload keeps the last
good version serving until the next change.

## Localisation

`wayrun.locale()` answers the launcher's resolved locale, the same stem its own
tables use, such as `en` or `zh_cn`, and it is available while the script loads,
so the plugin table's `name` and `description` can be localised too. Every `zh_*`
variant has already been folded into `zh_cn`; any other language arrives
normalised (`fr_fr`). The value is read once at startup, so changing it needs the
same restart the launcher does.

The plugin owns its catalogue and its fallback; the launcher never translates
plugin text. Keep the strings in a table keyed by locale and fall back to the one
you always ship:

```lua
local CATALOGS = {
  en = { description = "Search GitHub repositories", copy_url = "Copy HTML URL" },
  zh_cn = { description = "搜索 GitHub 仓库", copy_url = "复制 HTML 链接" },
}

local fallback = CATALOGS.en
local strings = CATALOGS[wayrun.locale()] or fallback
local function t(key)
  return strings[key] or fallback[key]
end
```

Translate the prose the plugin authors — identity name and description, row
titles and summaries, action titles, empty and error messages — and leave data
alone: repository names, file names, commands, URIs and clipboard text are not
translations. For a larger catalogue, put one JSON file per locale beside the
script and read it with `wayrun.fs.read` into `wayrun.json.decode`:

```lua
local dir = wayrun.script_dir() .. "/locales/"
local text = wayrun.fs.read(dir .. wayrun.locale() .. ".json")
  or wayrun.fs.read(dir .. "en.json")
local strings = wayrun.json.decode(text or "{}")
```

A host in another language reads the same value from `$WAYRUN_LOCALE`.

## Limits

- A Lua plugin may join the default chain with `keyword = ""`. Its rows are
  listed, not scored, so they rank below every scored provider row and only
  fill the remaining slots. The default chain bounds a keyword-less host with a
  short deadline and skips it when it overruns, so keep `search` cheap and
  return nothing when the query is not yours.
- Rows keep the order the script returns; there is no relevance channel.
- Icons follow the rule in [plugins.md](plugins.md#external-hosts): a file the
  script ships, a theme icon `wayrun.icon(name)` resolved, or a remote image as
  an `https://` URL: the core fetches and caches it in the background. The same
  URL works as an image preview's `path`; set `icon_shape = "circle"` for an
  avatar. Use `wayrun.plugin_cache_dir(id)` for other downloaded data, and
  `wayrun.fs.delete` to evict it.

## Registering

Copy `template.lua`, keep the shebang and the exec bit (`chmod +x`), and
register it like any host — a single executable token, with the fields in
[plugins.md](plugins.md#entry-fields) and the contract in
[jsonrpc.md](jsonrpc.md). The first line of the script must be
`#!/usr/bin/env -S wayrun --lua-host`.

For editor completion, point LuaLS's `Lua.workspace.library` at
[`lua/wayrun.lua`](../../lua/wayrun.lua); it declares the same `wayrun` table.

[firefox.lua](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/firefox.lua)
and [web.lua](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/web.lua)
are complete worked examples, reading `places.sqlite` and the web respectively.
