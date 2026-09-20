# JSON-RPC 2.0

The core (`wayrun --core`, or a `wayrun-core` symlink) also speaks
[JSON-RPC 2.0](https://www.jsonrpc.org/specification) over the same stdin/stdout.
Lines that parse to an object with `"jsonrpc":"2.0"` and a `method` are handled
as RPC requests and can be mixed with the launcher's text protocol. Responses
are newline-delimited JSON on stdout.

```sh
printf '%s\n' '{"jsonrpc":"2.0","method":"search","params":{"text":"firefox"},"id":1}' | wayrun --core
# -> {"jsonrpc":"2.0","result":[...],"id":1}
```

| Method | Params | Result |
|--------|--------|--------|
| `search` | `{"text"}` | array of result items |
| `top` | — | most-used items |
| `select` | item object | `null` (records usage; `ephemeral` and `copy:` rows are not) |
| `forget` | `{"on_click"}` | `{"forgotten": bool}` |
| `run` | `{"cmd"}` | `null` |
| `action` | `{"desktop_id","action_id"}` | `null` (runs one `[Desktop Action …]` group of a desktop file) |
| `launch` | `{"desktop_id"}` | `null` (launches through GLib's `GAppInfo`) |
| `open` | `{"uri"}` | `null` (opens with the default handler) |
| `reveal` | `{"uri"}` | `null` (shows a file in the file manager) |
| `pin` | `{"scope","item"}` | `{"pinned": bool}` (pins an item to an exact query) |
| `unpin` | `{"scope","on_click"}` | `{"unpinned": bool}` |
| `copy` | `{"text"}` | `null` (writes the Wayland clipboard) |
| `resolve_icon` | `{"name"}` | absolute path for an icon spec |
| `list_plugins` | — | plugin metadata; see [schema](#plugin-metadata-list_plugins) |
| `theme` | — | theme colors |
| `ping` | — | `"pong"` |

`search` takes an object with a `text` key (a non-empty string). An absent
`params`, an empty `text`, a bare string, `{"query": …}`, or a non-string `text`
returns `-32602`. Use `top` for the most-used items — `search` does not serve a
default view.

`forget` drops a row from usage history. When the `on_click` is a `run:` shell
command whose first token is a registered external host's `command` (absolute
path, or PATH-resolved when the config uses a bare name), the core also relays
a `forget` request to that host so it can delete its own data — e.g. the todo
plugin removes the todo. The answer says whether anything was really dropped:
`true` when a history row was deleted **or** a host that owns the row answered
without an error, `false` otherwise. A built-in provider has nothing to forget,
and a host without a `forget` method answers `-32601`, which counts as "not
mine" — the shell keeps such a row in the list rather than claiming a deletion
nobody made. The host walk runs in a task of its own, so a slow or wedged host
cannot hold the stdin loop; its reply simply lands later, carrying its `id`.

`pin` stores an item under an exact query string (`scope` is the whole trimmed
input; `""` is the empty-query history), keyed by its `on_click`; `unpin` removes
it. A later `search` whose `text` trims to that same string prepends the pins,
most recently pinned first, deduplicated against the fresh results, and decorates
them with their `actions`; a bare keyword does not match. The item JSON is
stored whole, because a pinned row is re-emitted before its plugin runs.

A request without an `id` is a notification (side effect only, no response).
Unknown methods return `-32601`; malformed requests `-32600`; bad params
`-32602`.

`resolve_icon` resolves any icon spec — an absolute path, a theme icon name, or
the `papirus:` scheme below — to the absolute path the shell renders. It
is meant for external plugin hosts that build icons dynamically (e.g. a
`list_plugins` identity) without hard-coding theme paths. An absent or
non-string `name` returns `-32602`.

## Plugin metadata (`list_plugins`)

`list_plugins` returns an array of plugin objects. There are two shapes:

### Core → client

Called on the core's stdin, it answers with the current registry:

| Key | Type | Meaning |
|-----|------|---------|
| `id` | string | plugin id, matches the `plugins.toml` entry |
| `name` | string | display name |
| `icon` | string | icon path or theme name (external hosts' `papirus:` specs are already resolved to absolute paths) |
| `keyword` | string | trigger prefix (empty = default) |
| `enabled` | bool | whether the plugin is active |

### External host → core (identity discovery)

When a `plugins.toml` entry declares `command`, the core spawns the host and
calls `list_plugins` once to discover identity. A Python framework and example
hosts that speak this contract live in the
[WayRun-Plugins](https://github.com/Prslc/WayRun-Plugins) workspace; the
response `result` is an array of objects:

| Key | Type | Meaning |
|-----|------|---------|
| `id` | string (required) | plugin id — must match the `plugins.toml` entry id, or the identity is ignored |
| `name` | string | display name (empty → falls back to the configured id) |
| `icon` | string | absolute path or `papirus:` spec (see [Icon specs](#icon-specs)) |
| `description` | string | ready hint shown in the `?` list and the keyword+space hint |

## Default views for keyword plugins

A `plugins.toml` entry with a non-empty `keyword` is opened by a query that is
just the keyword followed by a space (e.g. `todo `) — through the text protocol
and the `search` RPC alike. Opening asks the external host for its **default
view**:

```sh
printf '%s\n' '{"jsonrpc":"2.0","method":"top","params":{"plugin":"todo"},"id":1}' | /path/to/todo/main.py
# -> {"jsonrpc":"2.0","result":[{"title":…,"summary":…,"on_click":…,"icon":…}],"id":1}
```

This `top` call is a core → host request — distinct from the core's own `top`
RPC (most-used usage history), which serves the empty-launcher view. A host
declares a default view by serving `top` (registering it via the plugin
framework's `@plugin.method("top")`); the response `result` is an array of
result items with the same [schema](#result-items) as `search`, icons
included.

When the host returns a non-empty default view it is shown instead of the
keyword+space identity hint. The hint stays otherwise:

- host without `top` (unknown method `-32601`) or a failing handler
  (`-32603`) → identity card from `list_plugins` (`name` + `description`)
- empty result list → same identity card, as the plugin's empty state

## Result items

`search` and `top` return an array of items. Every item is an object with these
keys — the first five are always present (`null` for an absent optional field),
and `actions`/`badge` only when set:

| Key | Type | Meaning |
|-----|------|---------|
| `title` | string | primary label (app name, command, file name, …) |
| `summary` | string \| null | secondary line (command, path, description, …) |
| `on_click` | string \| null | action bound to Enter; see the schemes below |
| `icon` | string \| null | absolute path to an icon image; see [Icon specs](#icon-specs) |
| `ephemeral` | bool | when true, selecting this row is not recorded in usage history |
| `actions` | array | optional secondary commands for the shell's `Shift+Enter` panel |
| `badge` | string \| null | optional status glyph at the row's right edge (a pin for a pinned row) |

An `actions` entry is `{"title": string, "on_click": string, "icon"?: string}`,
with the same icon-spec resolution as a row's `icon`. The core attaches the
launcher-level pin/unpin to every actionable row, and history removal to a row
it sourced from the empty-query history that could have been recorded (not
`ephemeral`, not `copy:`); the owning built-in
provider adds its type-specific ones (a file reveal, a `[Desktop Action …]`
group, a copy-link), and a host's own entries are kept after them. External hosts
may emit `actions` directly on a result; the shell renders them without knowing the
scheme. The panel-only schemes are `pin:{"scope","item"}`,
`unpin:{"scope","on_click"}`, `forget:<on_click>` and `reveal:<uri>`; every row
scheme (`run:`, `launch:`, `copy:`, `action:`, a URL) also works.

`on_click` schemes:

| Scheme | Effect |
|--------|--------|
| `run:<shell cmd>` | execute a shell command (system commands, clipboard) |
| `launch:<desktop-id>` | launch an app by desktop id (app-search) |
| `copy:{"text":"…"}` | write the text to the Wayland clipboard (translate copy) |
| `action:<desktop-id>:<action-id>` | run a desktop action; the shell forwards it as the `action` command (app-search emits one row per action, DMS-style) |
| bare URL / `file:` / `mailto:` URI | opened by the core with GLib `g_app_info_launch_default_for_uri` |

File URIs are percent-encoded, so paths with spaces or non-ASCII characters
survive; a `Terminal=true` handler is started inside a terminal.

An item without `on_click` is non-interactive (display only).

Selecting an item records it in usage history — the list behind an empty query
(`top`). Two kinds of row are exempt: one the host marked `ephemeral: true` (a
one-shot search hit, say), and one whose `on_click` is a `copy:` write (its
value is the copied text, not a target to re-open). The field or scheme
declares the semantics, so the rule holds for every source — built-in provider
and external host alike.

### Icon specs

The shell renders icons as `file://` + path, so every `icon` the core
emits is an absolute path. **External plugin hosts** (a `plugins.toml` entry
with `command`) may return a theme icon name, a `papirus:` spec or an absolute
path — in any result-item `icon` field (`search` results and `top` default views
alike) and in the `list_plugins` identity `icon` — and the core resolves it
before the item reaches the shell:

| Spec | Resolution |
|------|------------|
| absolute path | passed through |
| theme name | searched under `$XDG_DATA_HOME/icons`, each `$XDG_DATA_DIRS/icons`, `~/.icons` and `/usr/share/pixmaps`, user dirs first |
| `papirus:<name>` | first match for `<name>` across Papirus categories and sizes |
| `papirus:<category>/<name>` | same, but searches `<category>` first |

Examples: `papirus:folder-open` →
`/usr/share/icons/Papirus/48x48/places/folder-open.svg`;
`papirus:apps/firefox` → `/usr/share/icons/Papirus/48x48/apps/firefox.svg`.
