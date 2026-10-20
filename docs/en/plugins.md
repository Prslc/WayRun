# Plugins

`$XDG_CONFIG_HOME/wayrun/plugins.toml` (default
`~/.config/wayrun/plugins.toml`) is the plugin registry. It is generated on the
first run as a commented template and is watched, so an edit is picked up without
restarting.

`wayrun plugin list` prints the effective registry — ids, names, keywords, icons
and any disabled entry — with the user file already merged over the built-ins.

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

A provider that cannot score an item (an external host) lists its rows instead:
they sit below every scored row and fill the remaining slots in the provider's
own order. A keyword-less external host is bounded by a short deadline in that
chain, so a stalled host never delays the card.

## Built-in plugins

| Id | Keyword | What it searches | Needs |
| --- | --- | --- | --- |
| `calculator` | `""` | Inline arithmetic. | — |
| `system-commands` | `""` | `lock`, `reboot`, `shutdown`, `suspend`, `logout`. | — |
| `app-search` | `""` | Installed applications (desktop entries): the name, a near-spelling of it, and each metadata surface at the strength it can carry (a keyword or generic name by word, a description by prefix). | — |
| `path-lookup` | `""` | A path query (`~…`, `/…` or containing `/`): the existing path itself, or the typed directory's matching entries; also the first `$HOME` level when no provider matches a plain query. | — |
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
"Reveal in file manager" and "Copy path", then the system's other handlers for
the content type ("Open with …"); `app-search` lists the entry's
`[Desktop Action …]` groups, and an external host's rows carry whatever `actions`
it attaches (the workspace's `web.lua` offers "Copy URL"). The row's own command
leads the panel, and an external host's own `actions` follow the plugin's.

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
crashes or stalls; the next call starts a new one. An edit to the configured
script is picked up on the next call too. The host must be stateless per call
either way, but it may cache within its process lifetime.

The identity `icon` and an action's `icon` must be an absolute path: a file the
host ships itself (`wayrun.script_dir()` locates the script's own directory) or
a theme icon the Lua SDK's `wayrun.icon()` resolved. A result's own `icon` is an
absolute path too, or an `https://` URL: the core fetches it in the background
into its icon cache and hands the shell a local path, so the row draws at once
and fills in when the image lands. A `builtin:` glyph, or a theme name or
`papirus:` spec left unresolved, counts as no icon. A result whose own icon is
missing falls back to the plugin's identity icon, and the built-in placeholder
answers when that is missing too. A row may set `icon_shape = "circle"` to clip
its icon box, as an avatar needs.

A plugin that needs other network data (an API cache, a non-image file) still
fetches and caches it itself; result icons and preview images are the core's.

A host's `search` error is shown as one display-only row whose summary is the
first line of the error message; a `top` error keeps the identity card.

A host inherits the launcher's resolved locale as `WAYRUN_LOCALE`, the stem its
own tables use (`en`, `zh_cn`, …); it owns its catalog and fallback, and the
launcher never translates plugin text. [lua.md](lua.md#localisation) has the Lua
pattern.

Every plugin names the API version it targets — a host in its `list_plugins`
identity, a Lua script in its plugin table — and the launcher logs a mismatch
against its own version without refusing the plugin, which may still work. The
number moves when a plugin can observe the change (a binding, a signature, a
field, a sandbox rule), never for an addition or for a change inside the
launcher, so gaining `wayrun.crypto` does not move it while `wayrun.env` gaining
a scope does. A plugin written before versioning is `0`, so it is warned about
until it states the version it means. Lua can read the current one at runtime
through `wayrun.api`.

Hosts can be written by hand. The
[WayRun-Plugins](https://github.com/Prslc/WayRun-Plugins) workspace ships a
Python framework, examples and templates (`template/`, `template.lua`). A Lua
plugin is first-party — the binary hosts the script itself, see [lua.md](lua.md)
— and the host protocol is the JSON-RPC subset documented in
[jsonrpc.md](jsonrpc.md): `search`, `top`, `list_plugins`.
