# Plugins

`$XDG_CONFIG_HOME/wayrun/plugins.toml` (default
`~/.config/wayrun/plugins.toml`) is the plugin registry. The first run writes
it from the shipped defaults, and the launcher watches it, so an edit applies
without a restart.

## Registry commands

`wayrun plugin list` prints the effective registry: ids, names, keywords, icons
and any disabled entry. It merges your file over the built-ins.

- `wayrun plugin add <script.lua> [--keyword K] [--resident]` registers a Lua
  host. It loads the script and refuses a missing shebang or exec bit, an id
  already configured, or a keyword another enabled entry owns. It then appends
  the entry; the keyword defaults to the plugin id.
- `wayrun plugin enable|disable <id>` writes or edits the user entry for an id,
  built-ins included.
- `wayrun plugin set-keyword <id> <keyword>` changes the routing keyword the
  same way. An empty keyword puts the plugin in the default chain.
- `wayrun plugin remove <id>` drops a user entry, leaving a built-in at its
  shipped default.

Every edit keeps the file's comments and is written at once. A running launcher
picks the file up through its watcher.

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

Reorder entries to change priority. An id that is neither a built-in nor a host
is ignored.

## Routing

The first word routes to a plugin only when some plugin owns that keyword.
Otherwise the whole input, first word included, is the default providers'
query: `foo bar` reaches a default provider as `foo bar`, not `bar`.

Every default provider (keyword `""`) answers, and the launcher merges their
rows in three steps. It first ranks by relevance: the match kind's weight,
scaled by the surface that matched. It then breaks ties by usage count, and
finally by entry order. A title match therefore leads a description match of
comparable strength. An exact keyword still beats a title that only contains
the query.

A provider that cannot score an item, such as an external host, lists its rows
instead. They sit below every scored row and fill the remaining slots in the
provider's own order. In the default chain a keyword-less external host is
bounded by a quarter-second deadline, so a stalled host never delays the card.

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

A keyword whose dependency is absent returns no rows instead of failing.

`file-search` and `path-search` cover the whole home directory through an
index. `config.md`'s `[files] index = false` restricts them to a few levels
under `$HOME`; they then keep no cache.

## Result actions

A row can carry secondary commands, shown in the `Shift+Enter` panel. They
differ by result type:

- `file-search`, `path-search` and `path-lookup` offer "Open in terminal",
  "Reveal in file manager" and "Copy path", then an **Open with** group for the
  system's other handlers, which opens a further panel level.
- `app-search` lists the entry's `[Desktop Action …]` groups.
- An external host's rows carry only whatever `actions` it attaches; the
  built-in file commands are not added. The workspace's `web.lua`, for example,
  offers "Copy URL".

The row's own command leads the panel. Actions belong to the plugin that
produced the row, so a remembered default for one provider never marks another
provider's identical action.

Give an action an `id` to let the user make it the default `Enter` action with
`Alt+Enter`. The panel's **Open** entry, the row's own command, takes the
gesture as clearing the default. See the `actions` field in
[jsonrpc.md](jsonrpc.md#result-items).

## External hosts

An external host is a separate executable that `command` names and that
answers JSON-RPC over stdin and stdout.

### Command and timeouts

`command` names a JSON-RPC 2.0 host: a single executable token, resolved on
`PATH` or given as an absolute path. It takes no arguments and no shell syntax,
so a script needs a shebang and the exec bit. Each call starts the host fresh
and is bounded by a short timeout, so a stalled host cannot hang the launcher.

### Resident hosts

`resident = true` keeps one host process alive across calls. A call then costs
about a millisecond, against several for a fresh process. The host is dropped
when the launcher dismisses, when it sits idle for two minutes, or when a call
crashes or stalls; the next call starts a new one. An edit to the configured
script is picked up on the next call.

The host must be stateless per call either way, but it may cache within its
process lifetime.

### Icons and previews

The identity `icon` and an action's `icon` must be an absolute path: a file the
host ships itself (`wayrun.script_dir()` locates the script's own directory) or
a theme icon that the Lua SDK's `wayrun.icon()` resolved.

A result's own `icon` is an absolute path, or an `https://` URL. The core
fetches a URL in the background into its icon cache and hands the shell a local
path, so the row draws at once and fills in when the image lands.

An unresolved theme name, an unresolved `papirus:` spec, or a `builtin:` glyph
counts as no icon. If a result has no icon, the launcher uses the plugin's
identity icon. If that is missing too, it draws the bundled placeholder. A row
may set `icon_shape = "circle"` to clip its icon box, as an avatar needs.

A plugin that needs other network data, such as an API cache or a non-image
file, still fetches and caches it itself. Result icons and preview images
belong to the core.

### Errors

A host's `search` error is shown as one display-only row whose summary is the
first line of the error message. A `top` error keeps the identity card.

### Locale

A host inherits the launcher's resolved locale as `WAYRUN_LOCALE`, the stem its
own tables use (`en`, `zh_cn`, …). It owns its catalog and fallback; the
launcher never translates plugin text. [lua.md](lua.md#localisation) has the Lua
pattern.

### API version

Every plugin names the API version it targets: a host in its `list_plugins`
identity, a Lua script in its plugin table. The launcher logs a mismatch
against its own version, but still loads the plugin, which may work.

The number changes only when a plugin can observe the change: a binding, a
signature, a field or a sandbox rule. An addition does not move it: gaining
`wayrun.crypto` leaves the number alone, while `wayrun.env` gaining a scope
moves it. A plugin written before versioning is `0`, and is warned about until
it states the version it means. Lua can read the current one at runtime through
`wayrun.api`.

## Network and proxy

The core makes no outbound requests itself. A registered host that fetches
does, such as the WayRun-Plugins `web.lua`. Its
[`wayrun.http.get`](lua.md#api) reads the launcher's environment,
so set a proxy in that environment or with `Environment=` in the systemd unit:

```ini
# HTTP CONNECT proxy; user:password@ optional
Environment=https_proxy=http://127.0.0.1:7890
Environment=http_proxy=http://127.0.0.1:7890
```

Then run `systemctl --user daemon-reload && systemctl --user restart
wayrun-launcher`.

The launcher reads `ALL_PROXY`, then `HTTPS_PROXY`, then `HTTP_PROXY`, in either
letter case. The first usable value wins, and an unusable value is skipped, so
a broken `https_proxy` does not hide a working `http_proxy`.

`NO_PROXY` is honoured per request, with exact, suffix and wildcard patterns.
Write the scheme out: `http://` is an HTTP CONNECT proxy on port 80 and
`https://` one on 443. A scheme-less value means HTTP, not SOCKS. `socks5://`
is not in the shipped build. With no usable value, requests go direct.

## Writing a host

Hosts can be written by hand. The
[WayRun-Plugins](https://github.com/Prslc/WayRun-Plugins) workspace ships a
Python framework, examples and templates (`template/`, `template.lua`).

A Lua plugin is first-party: the binary hosts the script itself. See
[lua.md](lua.md). The host protocol is the JSON-RPC subset documented in
[jsonrpc.md](jsonrpc.md): `search`, `top` and `list_plugins`. Worked examples
are in the [Plugin directory](plugins-directory.md).
