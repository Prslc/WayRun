# Plugins

`~/.config/wayrun/plugins.toml` is the plugin registry. It is generated on the
first run as a commented template and is watched, so an edit is picked up without
restarting.

## Entry fields

```toml
[[plugins]]
id = "runner"
keyword = "r"
enabled = true
```

| Field | Required | Meaning |
| --- | --- | --- |
| `id` | yes | Which plugin this entry configures: a built-in id, or the id an external host reports. |
| `keyword` | yes | The prefix that routes input to this plugin. `""` makes it a **default** provider. |
| `enabled` | no | Defaults to `true`. `false` disables the plugin without removing the entry. |
| `command` | no | An external JSON-RPC 2.0 host. See below. |
| `resident` | no | Defaults to `false`. Keep the host process alive across calls. See below. |

Reorder entries to change priority. An `id` that is neither a built-in nor a host
is ignored.

## Routing

The first word of the input routes to a plugin only when some plugin owns that
keyword. Otherwise the whole input, first word included, is the default
providers' query: `foo bar` reaches a default provider as `foo bar`, not `bar`.

Default providers (keyword `""`) all answer, and their rows are merged: by each
row's relevance — the match kind's weight, scaled by the surface it matched on —
then by the row's usage count, then by entry order. A title match therefore leads
a description match of comparable strength, while an exact keyword still beats a
title the query only sits inside of.

## Built-in plugins

| Id | Keyword | What it searches | Needs |
| --- | --- | --- | --- |
| `calculator` | `""` | Inline arithmetic. | — |
| `system-commands` | `""` | `lock`, `reboot`, `shutdown`, `suspend`, `logout`. | — |
| `app-search` | `""` | Installed applications (desktop entries): the name, a near-spelling of it, and each metadata surface at the strength it can carry (a keyword or generic name by word, a description by prefix). | — |
| `runner` | `r` | Fuzzy `$PATH` executables; accepts arguments. An installed app launches through GLib (honoring `Terminal=`), anything else runs in a terminal. | a terminal emulator |
| `file-search` | `f` | Files under the home directory, at any depth. | — |
| `path-search` | `d` | Directories under the home directory, at any depth. | — |
| `clipboard` | `c` | Clipboard history. | `cliphist` running |
| `window` | `w` | Open windows (niri or Hyprland, per build). | the matching compositor backend |

A keyword whose dependency is absent returns no rows instead of failing.

`file-search` and `path-search` cover the whole home directory through an index.
`config.md`'s `[files]` `index = false` restricts them to the configured `depth`
instead; they then keep no cache.

## Result actions

A result row can carry secondary commands, shown in the `Shift+Enter` panel. They
differ by result type: `file-search`/`path-search` offer "Open in terminal",
"Reveal in file manager" and "Copy path", `app-search` lists the entry's
`[Desktop Action …]` groups, an external host's rows carry whatever `actions` it
attaches (the workspace's `web.lua` offers "Copy URL"), and every actionable row
gets the launcher-level pin/unpin — plus "Remove from history" on a row the
empty-query history sourced. The row's own command leads the panel, an external
host's own `actions` follow the plugin's, and the launcher-level entries come
last.

Give an action an `id` to let the user make it the default `Enter` action with
`Alt+Enter`; the panel's **Open** entry — the row's own command — takes the
gesture as clearing it. See the `actions` field in
[jsonrpc.md](jsonrpc.md#result-items).

## External hosts

`command` names a JSON-RPC 2.0 host: a single executable token, resolved on
`PATH` or given as an absolute path. It takes no arguments and no shell syntax,
so a script needs a shebang and the exec bit. Each call starts the host fresh and
is bounded by a short timeout, so a stalled host cannot hang the launcher.

`resident = true` keeps one host process alive across calls instead — about a
millisecond per call against the fresh process's several. The host is dropped
when the launcher dismisses, when it sits idle for two minutes, or when a call
crashes or stalls; the next call starts a new one. The host must be stateless
per call either way, but it may cache within its process lifetime.

Both the identity `icon` and each result `icon` must be an absolute path to an
icon file the host ships itself; the same holds for an action's `icon` and a
row's `badge`. A theme icon name, a `papirus:` spec or a `builtin:` glyph counts
as no icon. A result whose own icon is missing falls back to the plugin's
identity icon, and the built-in placeholder answers when that is missing too.

Hosts can be written by hand. The
[WayRun-Plugins](https://github.com/Prslc/WayRun-Plugins) workspace ships a
Python framework, Lua and Python examples and templates (`template/`,
`template.lua`), and the host protocol is the JSON-RPC subset documented in
[jsonrpc.md](jsonrpc.md): `search`, `top`, `forget`, `list_plugins`.
