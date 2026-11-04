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
| `record` | `{"on_click": Action}` | `null` (bumps that command's usage count; a `copy` command is not recorded) |
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
| `results_partial` | array of result items | a streaming search's intermediate payload; the search stays pending |
| `image_ready` | `{"path": "…"}` | a background image fetch landed; the UI reloads that path |

`search` sent **without** an `id` is streaming: the core aborts any pending
search, then answers with a `results` notification. A streaming host may emit
`results_partial` notifications before that final payload. Sent **with** an `id`
it answers synchronously with the array, for one-shot clients.

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
| `open_with` | `desktop_id`, `uri` | open a URI with one named handler instead of the default |
| `reveal` | `uri` | show a file in the file manager (panel-only) |
| `terminal` | `uri` | open a terminal in the URI's directory, its parent for a file (panel-only) |
| `plugin` | `plugin`, `action`, `payload` | hand a named command and its payload to that plugin's host |

`plugin`'s payload is an arbitrary JSON value the named plugin interprets; the
core only routes it, and the host runs it in the plugin's own scope (see
[Commands](#commands)).

`run`'s `cmd` is a whole shell line, and nothing in it is rewritten: a
`%u`-style field code reaches the shell as typed, since only `launch` and
`desktop_action` read `.desktop` field codes; those two carry an unquoted id.
File URIs are percent-encoded, so paths with spaces or non-ASCII characters
survive; a `Terminal=true` handler is started inside a terminal.

`search` takes an object with a `text` key (a non-empty string). An absent
`params`, an empty `text`, a bare string, `{"query": …}`, or a non-string `text`
returns `-32602`. A notification with an empty `text` cancels any pending
search, so a cleared field never receives a stale payload.

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
[WayRun-Plugins](https://github.com/Prslc/WayRun-Plugins) workspace; a Lua plugin
is hosted by the launcher itself and answers the same call — see
[lua.md](lua.md). The response `result` is an array of objects:

| Key | Type | Meaning |
|-----|------|---------|
| `id` | string (required) | plugin id — must match the `plugins.toml` entry id, or the identity is ignored |
| `name` | string | display name (empty → falls back to the configured id) |
| `icon` | string | an absolute path to an icon the host ships (see [Icon specs](#icon-specs)) |
| `description` | string | ready hint shown in the `?` list and the keyword+space hint |
| `api` | number | the plugin API the host targets; absent means `0`, before versioning |

`api` is compared against the launcher's own plugin API version. A mismatch is
logged on stderr and never refuses the plugin, which may still work; the rule for
when the number moves is in [plugins.md](plugins.md).

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

## Commands

A row whose `on_click` (or action) is `{"type":"plugin","plugin":"todo", …}`
sends the named command back to its host instead of running anything on the
core's side:

```sh
printf '%s\n' '{"jsonrpc":"2.0","method":"command","params":{"plugin":"todo","action":"done","payload":{"id":7}},"id":1}' | /path/to/todo/main.py
# -> {"jsonrpc":"2.0","result":null,"id":1}
```

The action name and payload are the plugin's own; the core routes them and runs
the handler in the plugin's scope, so a Lua script's `fs.write`, `env` and
`exec` work as they do in a search. The launcher is already dismissing when the
command runs, so a failure reaches the journal, not the list; the reply itself
is ignored.

## Streaming searches

A core that supports streaming sends `"stream": true` in a `search` request's
params. A host may then send any number of notifications before its reply:

```json
{"jsonrpc":"2.0","method":"partial","params":{"id":17,"rows":[ … ]}}
```

`rows` is a result-item array with the same schema as the reply, and `id` is the
request's own id. Each notification replaces the rows on screen; the final reply
is authoritative and ends the search. A host that does not stream omits them,
and an older core never sends `stream`, so a host treats the absent flag as
"answer once".

## Result items

`search`, and a host's `top`, return an array of items. Every item is an object
with these keys — `title`, `summary`, `on_click` and `icon` are always present
(`null` for an absent optional field); `preview`, `actions` and `icon_shape`
only when set:

| Key | Type | Meaning |
|-----|------|---------|
| `title` | string | primary label (app name, command, file name, …) |
| `summary` | string \| null | secondary line (command, path, description, …) |
| `on_click` | [`Action`](#actions) \| null | action bound to Enter |
| `icon` | string \| null | the icon spec, an absolute path or a `builtin:` glyph; a host's result may be an `https://` URL the core caches; see [Icon specs](#icon-specs) |
| `icon_shape` | string | optional; `"circle"` clips the icon box to a circle, absent draws the file as it is |
| `preview` | object | optional split-view payload, `{"type":"image","path":"/abs/file.png"}` or `{"type":"text","text":"…"}`; a host's image path may be an `https://` URL |
| `actions` | array | optional secondary commands for the `Shift+Enter` action panel |

A `preview` is tagged by `type` like an `Action`: `image` carries an absolute
path or, from a host, an `https://` URL the core fetches into its image cache
(PNG, SVG, JPEG, WebP, GIF or BMP), and is shown in the right column for the
selected row; `text` carries plain UTF-8, wrapped by the launcher and capped at
64 KiB. A theme name or a `builtin:` glyph is not a preview.

An `actions` entry is either a command,
`{"title": string, "action": Action, "icon"?: string}`, or a group,
`{"title": string, "children": [ …entries… ]}`, which Enter opens as a further
level of the action panel. Icons use the same resolution as a row's `icon`. An
entry may also carry `id` (its stable kind), `plugin` (the owner, which scopes a
remembered default) and `default` (true when Enter runs it). The core assigns
`plugin` and `default` — a host's values for those are ignored — and a host sets
`id` on its own actions to make them defaultable, a grouped entry included.

The owning built-in provider adds its type-specific entries (a file reveal, copy
path or open in terminal, the system's other handlers for the content type, a
`[Desktop Action …]` group, a copy-link), and a host's own entries are kept
after them.

An item without `on_click` is non-interactive (display only).

`record` bumps the launch count of the command `on_click` names; the counts rank
the default providers' results where they otherwise tie. A `copy` command is
exempt: its value is the copied text, not a target to re-open.

### Icon specs

Every `icon` the shell receives is an absolute path, or a `builtin:<name>` glyph
the launcher draws from its compiled set. **External plugin hosts** (a
`plugins.toml` entry with `command`) may return a result's own `icon` as an
`https://` URL: the core fetches it in the background into
`$XDG_CACHE_HOME/wayrun/icons` and hands the shell the cached path. A preview's
image `path` may be an `https://` URL handled the same way. The identity `icon`,
an action's `icon`, and a preview's local path remain files the host ships. A
`builtin:` glyph, a theme icon name or a `papirus:` spec counts as no icon.

A row icon that is missing or symbolic falls back to the plugin's identity icon;
when that is missing too, the bundled placeholder is drawn.
