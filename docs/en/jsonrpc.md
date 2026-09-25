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
| `top` | — | most-used items; sent as a notification, streams a `results` notification |
| `dismiss` | — | `null` (the launcher closed: drops the remembered payload and any search still in flight) |
| `select` | item object | `null` (records usage; `ephemeral` and `copy` rows are not) |
| `command` | an [`Action`](#actions) object | `null` (runs one row or panel command) |
| `pin` | `{"scope","on_click": Action}` | `{"pinned": bool}` (pins a row of that query's payload) |
| `unpin` | `{"scope","on_click": Action}` | `{"unpinned": bool}` |
| `default` | `{"scope","action_id"}` | `null` (remembers the default Enter action for a plugin; a null `action_id` clears it) |
| `forget` | `{"on_click": Action}` | `{"forgotten": bool}` |
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

`search` and `top` sent **without** an `id` are streaming: the core aborts any
pending search, then answers with a `results` notification. Sent **with** an
`id` they answer synchronously with the array, for one-shot clients.

## Actions

An `Action` is one object tagged by its `type` field (`{"type": …}`), describing
what a row runs. It is the params of the `command` method and the type of a
result's `on_click` and of a panel `execute` action:

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

`run`'s `cmd` is a whole shell line; `launch` and `desktop_action` carry an
unquoted id. File URIs are percent-encoded, so paths with spaces or non-ASCII
characters survive; a `Terminal=true` handler is started inside a terminal.

`search` takes an object with a `text` key (a non-empty string). An absent
`params`, an empty `text`, a bare string, `{"query": …}`, or a non-string `text`
returns `-32602`. Use `top` for the most-used items — `search` does not serve a
default view.

`forget` drops a row from usage history. When the `on_click` is a `run` action
whose first token is a registered external host's `command`, the core also relays
a `forget` request to that host so it can delete its own data — e.g. the todo
plugin removes the todo. The answer says whether anything was really dropped:
`true` when a history row was deleted **or** a host that owns the row answered
without an error, `false` otherwise. A host without a `forget` method answers
`-32601`, which counts as "not mine" — the launcher keeps such a row in the list
rather than claiming a deletion nobody made.

`pin` stores a row under an exact query string (`scope` is the whole trimmed
input; `""` is the empty-query history), keyed by the command `on_click` names:
the core looks the row up in the payload it last emitted for that scope, so a
client never has to echo a row back, and a command that payload no longer holds
answers `pinned: false`. `unpin` removes a pin by the same key. A later `search`
whose `text` trims to that same string prepends the pins, most recently pinned
first, deduplicated against the fresh results, and decorates them with their
`actions`; a bare keyword does not match.

## Plugin metadata (`list_plugins`)

`list_plugins` returns an array of plugin objects. There are two shapes:

### Core → client

Called on the core's stdin, it answers with the current registry:

| Key | Type | Meaning |
|-----|------|---------|
| `id` | string | plugin id, matches the `plugins.toml` entry |
| `name` | string | display name |
| `icon` | string | icon absolute path |
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
| `icon` | string | absolute path to an icon the host ships (see [Icon specs](#icon-specs)) |
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

This `top` call is a core → host request — distinct from the core's own `top`
RPC (most-used usage history), which serves the empty-launcher view. A host
declares a default view by serving `top` (registering it via the plugin
framework's `@plugin.method("top")`); the response `result` is an array of
result items with the same [schema](#result-items) as `search`, icons
included.

When the host returns a non-empty default view it is shown instead of the
keyword+space identity hint. Otherwise — a host without `top` (unknown method
`-32601`), a failing handler (`-32603`), or an empty result list — the hint comes
from `list_plugins` (`name` + `description`), which is also the plugin's empty
state.

The core translates its own strings (action titles, the launcher's help line);
whatever a host sends is relayed as it is, so a host owns the language of its
identity and its rows.

## Result items

`search` and `top` return an array of items. Every item is an object with these
keys — the first five are always present (`null` for an absent optional field),
and `actions`/`badge` only when set:

| Key | Type | Meaning |
|-----|------|---------|
| `title` | string | primary label (app name, command, file name, …) |
| `summary` | string \| null | secondary line (command, path, description, …) |
| `on_click` | [`Action`](#actions) \| null | action bound to Enter |
| `icon` | string \| null | absolute path to an icon image; see [Icon specs](#icon-specs) |
| `ephemeral` | bool | when true, selecting this row is not recorded in usage history |
| `actions` | array | optional secondary commands for the `Shift+Enter` action panel |
| `badge` | string \| null | optional status glyph at the row's right edge (a pin for a pinned row) |

An `actions` entry is `{"title": string, "action": PanelAction, "icon"?: string}`,
with the same icon-spec resolution as a row's `icon`. An entry may also carry `id`
(its stable kind), `plugin` (the owner, which scopes a remembered default) and
`default` (true when Enter runs it). The core assigns `plugin` and `default` — a
host's values for those are ignored — and a host sets `id` on its own actions to
make them defaultable.

A `PanelAction` is one of:

| `type` | Fields | Meaning |
|--------|--------|---------|
| `execute` | `command` | run that [`Action`](#actions) |
| `pin` | `scope` | pin the row's command to an exact query |
| `unpin` | `scope`, `on_click` | unpin the command from an exact query |
| `forget` | `on_click` | drop the command from usage history |

The core attaches the launcher-level pin/unpin to every actionable row, and
history removal to a row it sourced from the empty-query history that could have
been recorded (not `ephemeral`, not `copy`); the owning built-in provider adds
its type-specific ones (a file reveal, copy path or open in terminal, a
`[Desktop Action …]` group, a copy-link), and a host's own entries are kept
after them.

An item without `on_click` is non-interactive (display only).

Selecting an item records it in usage history — the list behind an empty query
(`top`). Two kinds of row are exempt: one the host marked `ephemeral: true` (a
one-shot search hit, say), and one whose `on_click` is a `copy` action (its
value is the copied text, not a target to re-open).

### Icon specs

Every `icon` is an absolute path. **External plugin hosts** (a `plugins.toml`
entry with `command`) must return an absolute path to an icon file the host
ships itself: in any result-item `icon` field (`search` results and `top`
default views alike), in an action's `icon`, in the row's `badge`, and in the
`list_plugins` identity `icon`. A theme icon name, a `papirus:` spec or a
`builtin:` glyph counts as no icon.

A row icon that is missing or symbolic falls back to the plugin's identity icon;
when that is missing too, the bundled placeholder is drawn.
