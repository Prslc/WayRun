# Plugins

`~/.config/wayrun/plugins.toml` is the plugin registry. It is generated on the
first run as a copy of `core/default-plugins.toml` and is watched, so an edit is
picked up without restarting the core.

## Entry fields

```toml
[[plugins]]
id = "runner"
keyword = "r"
enabled = true
```

| Field | Required | Meaning |
| --- | --- | --- |
| `id` | yes | Which plugin this entry configures. A built-in id, or the id an external host reports. |
| `keyword` | yes | The prefix that routes input to this plugin. `""` makes it a **default** provider. |
| `enabled` | no | Defaults to `true`. `false` disables the plugin without removing the entry. |
| `command` | no | An external JSON-RPC 2.0 host. See below. |

Reorder entries to change priority. Unknown or removed ids are ignored. A
built-in id with no `command` uses the compiled-in plugin; an unknown id with no
`command` is skipped.

## Routing

The first word of the input routes to a plugin only when some plugin owns that
keyword. Otherwise the whole input, first word included, is the default
providers' query: `foo bar` reaches a default provider as `foo bar`, not `bar`.

Default providers (keyword `""`) are tried in entry order and the first one with
non-empty results wins, so a bare query is app/command search, not a union of
everything.

## Built-in plugins

| Id | Keyword | What it searches | Needs |
| --- | --- | --- | --- |
| `calculator` | `""` | Inline arithmetic. | — |
| `system-commands` | `""` | `lock`, `reboot`, `shutdown`, `suspend`, `logout`. | — |
| `app-search` | `""` | Installed applications (desktop entries). | — |
| `runner` | `r` | Fuzzy `$PATH` executables; accepts arguments. An installed app launches through GLib (honoring `Terminal=`), anything else runs in a terminal. | a terminal emulator |
| `firefox-bookmarks` | `b` | Firefox bookmarks. | Firefox with a profile |
| `firefox-history` | `h` | Firefox history. | Firefox with a profile |
| `web-search` | `s` | Web search suggestions (engine set in `config.toml`). | network access |
| `file-search` | `f` | Files under the home directory. | — |
| `path-search` | `d` | Directories under the home directory. | — |
| `clipboard` | `c` | Clipboard history. | `cliphist` running |
| `window` | `w` | Open windows (niri or Hyprland, per build). | the matching compositor backend |

A keyword whose dependency is absent returns no rows instead of failing.

## Result actions

A result row can carry secondary commands shown in the shell's `Shift+Enter`
action panel. The menu is defined by the plugin that owns the row, not by the
shell, so it differs by result type: `file-search`/`path-search` offer "Reveal
in file manager", "Copy path" and "Open in terminal", `app-search` lists the
entry's `[Desktop Action …]` groups,
`web-search` and the Firefox plugins offer "Copy URL", and a plugin with none
simply gets the launcher-level pin/unpin. The empty-query history adds "Remove
from history" to the rows it sourced. An external host may put its own `actions`
array on a result item; the core appends them after the built-ins. Give an
action an `id` to let the user make it the default Enter action (`Alt+Enter`).
See the `actions` field in [jsonrpc.md](jsonrpc.md#result-items).

## External hosts

`command` names a JSON-RPC 2.0 host. The value is a single executable token,
resolved on `PATH` or given as an absolute path. It takes no arguments and no
shell syntax, so a script needs a shebang and the exec bit.

The core spawns the host fresh for each call (one request, then its stdin
closes), bounded by a 5s timeout, so a stalled host costs the deadline and never
the session. The identity the host reports through `list_plugins` is cached by
the file's `(mtime, size)`, so a later start does not fork an unchanged host.

Both the identity `icon` and each result `icon` must be an absolute path to an
icon file the host ships itself; the same holds for an action's `icon` and a
row's `badge`. The core resolves nothing for a host: a theme icon name, a
`papirus:` spec or a `builtin:` glyph counts as no icon. A result whose own icon
is missing falls back to the plugin's identity icon, and the built-in
placeholder answers when that is missing too.

Hosts can be written by hand. The
[WayRun-Plugins](https://github.com/Prslc/WayRun-Plugins) workspace ships a
Python framework, example plugins, and a `template/` to copy from. The host
protocol is the JSON-RPC subset documented in
[jsonrpc.md](jsonrpc.md): `search`, `top`, `select`, `forget`, `list_plugins`.
