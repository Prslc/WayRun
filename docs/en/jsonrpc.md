# JSON-RPC 2.0

The core (`wayrun --core`, or a `wayrun-core` symlink) speaks
[JSON-RPC 2.0](https://www.jsonrpc.org/specification) over stdin/stdout, and
nothing else: every line must be one JSON-RPC message. A line that is not valid
JSON is answered with the standard `-32700` parse error. Responses and
notifications are newline-delimited JSON on stdout.

```sh
printf '%s\n' '{"jsonrpc":"2.0","method":"search","params":{"text":"firefox"},"id":1}' | wayrun --core
# -> {"jsonrpc":"2.0","result":[...],"id":1}
```

| Method | Params | Result |
|--------|--------|--------|
| `search` | `{"text"}` | array of result items; sent as a notification, streams a `results` notification |
| `dismiss` | — | `null` (the launcher closed: drops any search still in flight) |
| `select` | item object | `null` (records usage; a `copy` row is not) |
| `command` | an [`Action`](#actions) object | `null` (runs one row or panel command) |
| `default` | `{"scope","action_id"}` | `null` (remembers the default Enter action for a plugin; a null `action_id` clears it) |
| `list_plugins` | — | plugin metadata; see [schema](#plugin-metadata-list_plugins) |
| `theme` | — | theme colors |
| `ping` | — | `"pong"` |

A request without an `id` is a notification (side effect only, no response).
Unknown methods return `-32601`; malformed requests `-32600`; bad params
`-32602`; a non-JSON line `-32700`.

## Notifications

The core also pushes JSON-RPC notifications (no `id`):

| Method | Params | Meaning |
|--------|--------|---------|
| `theme` | theme colors | the resolved theme, on connect and on every palette change |
| `results` | array of result items | a search payload |

`search` sent **without** an `id` is streaming: the core aborts any pending
search, then answers with a `results` notification. Sent **with** an `id` it
answers synchronously with the array, for one-shot clients.

## Actions

An `Action` is one object tagged by its `type` field (`{"type": …}`), describing
what a row runs. It is the params of the `command` method, the type of a result's
`on_click`, and the type of a panel entry's `action`:

| `type` | Fields | Effect |
|--------|--------|--------|
| `run` | `cmd` | execute a shell command (one shell line) |
| `run_in_terminal` | `cmd` | run a shell command inside a terminal emulator |
| `launch` | `desktop_id` | launch an app by desktop id through GLib's `GAppInfo` |
| `copy` | `text` | write the text to the Wayland clipboard |
| `desktop_action` | `desktop_id`, `action_id` | run one `[Desktop Action …]` group |
| `open` | `uri` | open a URI with the default handler (a URL, `file:` or `mailto:`) |
| `reveal` | `uri` | show a file in the file manager (panel-only) |
| `terminal` | `uri` | open a terminal in the URI's directory, its parent for a file (panel-only) |

`run`'s `cmd` is a whole shell line, and nothing in it is rewritten: a
`%u`-style field code reaches the shell as typed, since only `launch` and
`desktop_action` read `.desktop` field codes; those two carry an unquoted id.
File URIs are percent-encoded, so paths with spaces or non-ASCII characters
survive; a `Terminal=true` handler is started inside a terminal.

`search` takes an object with a `text` key (a non-empty string). An absent
`params`, an empty `text`, a bare string, `{"query": …}`, or a non-string `text`
returns `-32602`.

## Plugin metadata (`list_plugins`)

`list_plugins` returns an array of plugin objects. There are two shapes:

### Core → client

Called on the core's stdin, it answers with the current registry:

| Key | Type | Meaning |
|-----|------|---------|
| `id` | string | plugin id, matches the `plugins.toml` entry |
| `name` | string | display name |
| `icon` | string | the identity icon spec: absolute path or `builtin:` glyph |
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
| `icon` | string | an icon the host ships, or a `builtin:` glyph (see [Icon specs](#icon-specs)) |
| `description` | string | ready hint shown in the `?` list and the keyword+space hint |

A host with no `list_plugins`, or with no id matching its entry, still works —
searches are forwarded and results parsed — but its identity falls back to the
configured id with no icon, so `?` and the keyword+space hint show the default
placeholder.

## Default views for keyword plugins

A `plugins.toml` entry with a non-empty `keyword` is opened by a query that is
just the keyword followed by a space (e.g. `todo `) — through the `search`
method and the streaming notification alike. Opening asks the external host for
its **default view**:

```sh
printf '%s\n' '{"jsonrpc":"2.0","method":"top","params":{"plugin":"todo"},"id":1}' | /path/to/todo/main.py
# -> {"jsonrpc":"2.0","result":[{"title":…,"summary":…,"on_click":…,"icon":…}],"id":1}
```

This `top` call is a core → host request: a host declares a default view by
serving `top` (registering it via the plugin framework's
`@plugin.method("top")`); the response `result` is an array of result items with
the same [schema](#result-items) as `search`, icons included.

When the host returns a non-empty default view it is shown instead of the
keyword+space identity hint. Otherwise — a host without `top` (unknown method
`-32601`), a failing handler (`-32603`), or an empty result list — the hint comes
from `list_plugins` (`name` + `description`), which is also the plugin's empty
state.

The core translates its own strings (action titles, the launcher's help line);
whatever a host sends is relayed as it is, so a host owns the language of its
identity and its rows.

## Result items

`search`, and a host's `top`, return an array of items. Every item is an object
with these keys — the first four are always present (`null` for an absent
optional field), and `actions` only when set:

| Key | Type | Meaning |
|-----|------|---------|
| `title` | string | primary label (app name, command, file name, …) |
| `summary` | string \| null | secondary line (command, path, description, …) |
| `on_click` | [`Action`](#actions) \| null | action bound to Enter |
| `icon` | string \| null | the icon spec, an absolute path or a `builtin:` glyph; see [Icon specs](#icon-specs) |
| `actions` | array | optional secondary commands for the `Shift+Enter` action panel |

An `actions` entry is
`{"title": string, "action": Action, "icon"?: string}`, with the same icon-spec
resolution as a row's `icon`. An entry may also carry `id` (its stable kind),
`plugin` (the owner, which scopes a remembered default) and `default` (true when
Enter runs it). The core assigns `plugin` and `default` — a host's values for
those are ignored — and a host sets `id` on its own actions to make them
defaultable.

The owning built-in provider adds its type-specific entries (a file reveal, copy
path or open in terminal, a `[Desktop Action …]` group, a copy-link), and a
host's own entries are kept after them.

An item without `on_click` is non-interactive (display only).

Selecting an item records it in the usage counts that rank results. A row whose
`on_click` is a `copy` action is exempt: its value is the copied text, not a
target to re-open.

### Icon specs

Every `icon` is an absolute path, or a `builtin:<name>` glyph the launcher
draws from its compiled set. **External plugin hosts** (a `plugins.toml` entry
with `command`) return an icon file the host ships itself, or a known
`builtin:` glyph — in any result-item `icon` field (`search` results and `top`
default views alike), in an action's `icon`, and in the `list_plugins` identity
`icon`. A theme icon name or a `papirus:` spec counts as no icon.

A row icon that is missing or symbolic falls back to the plugin's identity icon;
when that is missing too, the bundled placeholder is drawn.
