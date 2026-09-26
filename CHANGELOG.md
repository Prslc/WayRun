# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.0] - 2026-09-26

### Added

- A plugin can be one Lua script the launcher runs itself. `wayrun --lua-host
  <script.lua>` answers the external-host contract (`list_plugins`, `search`,
  `top`, `forget`) with Lua 5.4 compiled into the binary, so nothing has to be
  installed beside it, and a `plugins.toml` `command` entry points at the script
  like any other host, its shebang naming the mode. The script runs sandboxed: a
  curated global set (no `os`, `io`, `package`, `load` or `print`) and a
  `wayrun` table of bindings — `t()` for locale-correct strings, `icon`,
  `urlencode`, `json`/`toml` decoders, `sqlite` over an immutable snapshot of a
  database, `http.get`, a scoped `fs`, `time`, `env`, `home`, `cache_dir`,
  `web_search_engine`, `script_dir`, `plugin_dir` and `log`. The API guide lives
  in the WayRun-Plugins workspace (`docs/en/LUA.md`).
- `resident = true` keeps a host process alive across calls: about a
  millisecond per call against the fresh fork's several. A per-request deadline
  kills and restarts a stalled or crashed host, a call a newer one overtook is
  skipped so a keystroke burst cannot stack work on the pipe, and a host is
  reaped when the launcher dismisses or after two minutes idle; a resident
  host's stderr joins the journal, where a fork-per-call child's is nulled. See
  `plugins.md`.
- `wayrun.plugin_dir(id)` names `~/.config/wayrun/plugins/<id>/`, the directory
  a plugin keeps its files in, and `fs.read` reads a relative path under it,
  refusing an absolute path or a `..`; `wayrun.toml.decode` parses the plugin's
  own config file, the format the launcher's own configs are written in. A
  plugin id is validated as a plain name, since it becomes a directory root.

### Changed

- **Breaking**: `b`, `h` and `s` no longer ship. The Firefox and web plugins are
  Lua scripts in the WayRun-Plugins workspace now, registered as ordinary hosts
  with a `command` entry, and what remains in the launcher is the mechanism:
  `--lua-host`, the bindings and `resident`. See `plugins.md`.
- **Breaking**: `wayrun.http.get` answers `{status, body}` instead of the bare
  body string, and takes a `headers` sub-table beside the query parameters. A
  script that read the body must read `res.body`; the headers are there because
  minreq sends no `User-Agent` of its own, so an API that requires one
  (GitHub's) was unreachable from a plugin while the Python SDKs, whose urllib
  sends one by default, never saw the problem.
- Host rows parse from the reply without a per-item clone: measured on the real
  Firefox payloads, the bookmark query takes 1.56 → 1.36 ms and the history
  query 2.48 → 2.35 ms, and every Python host call gains the same slice.

### Fixed

- `ui.locale` governs the text the `.desktop` files contribute (application
  names, their comments and desktop action labels), not just the launcher's own
  strings: the core pins the configured locale into `LANGUAGE` as well, which is
  the variable glib reads its language from, and the executor hands every app it
  launches the session's original value back. See `config.md` and `usage.md`.

## [0.5.0] - 2026-09-26

### Added

- Every spawn in `executor.rs` goes through `systemd-run --user --scope
  --collect --slice=app.slice`, so an app the launcher starts is no longer
  accounted to `wayrun-launcher.service`: the unit's memory and CPU stay its own,
  a restart cannot strand apps as `left-over process`, and systemd-oomd's victim
  is the app rather than the launcher plus everything it ever started. Expansion
  stays off (`--expand-environment=no`), since systemd-run's default rewrites a
  literal `${HOME}` in a command line or URI; the gio paths go through `gio
  launch`/`gio open` so the process gio forks lands in the scope too, and a spawn
  now hands the child to a task that waits it, so it no longer lingers as a
  zombie. Without a reachable user manager or the gio CLI the spawn keeps its
  in-process call, and apps launched before this release keep their old cgroup —
  a scope is not retroactive. See `resident.md`.
- The `dismiss` method: the launcher says it has closed, and the core cancels
  the search still in flight and drops the payload a `pin` names rows in. A
  `results` payload that lands after a dismissal no longer refills the rows or
  warms their icons.

### Changed

- **Breaking**: `[files] depth` is gone. It only mattered with the index off, so
  the fallback walk owns it as a constant now; a config still carrying the key
  parses and the key is ignored. An absent `exclude` now means the shipped
  `node_modules`, `target` and `__pycache__` rather than no exclusion at all, a
  user's own list replaces those names whole, and `exclude = []` searches
  everything but hidden names. A `[files]` section that set `depth` or relied on
  omitting `exclude` should be reviewed. See `config.md`.
- **Breaking**: `pin` takes `{"scope","on_click": Action}`, the pair `unpin`
  already took, and no longer accepts an `item` payload: the core looks the row
  up in the payload it last emitted for that scope, so a client names the row by
  the command it carries, and a command that payload no longer holds answers
  `pinned: false` instead of storing a stale row. Every row used to embed a copy
  of itself in its pin entry, and that copy crossed the wire twice; on a 20k-row
  history the `top` payload fell 17,131,343 → 13,004,661 bytes (a row 942 → 678,
  its pin entry 349 → 116) and peak RSS 74–76 → 69.6 MB, at 0.13 s wall either
  way. What a pin stores is unchanged. See `jsonrpc.md`.
- The file index loads lazily again, on the first `f`/`d`: pre-warming it cost
  every launcher open 32 MB of page cache and a 33 MB mapping whether or not the
  index was ever wanted. An index found stale now answers the query that found it
  stale instead of falling back to the depth-3 walk, which buys recall (hundreds
  of shallow rows against tens of thousands, once the image is a minute old) at
  the price of row data up to ~0.1 s out of date.
- The interactive path, by effect: a clipboard or window search answers from the
  last fetch and revalidates in the background, where `niri msg -j windows`
  (~12 ms) and `cliphist list` (~4 ms) used to run on every search past the
  500 ms burst window; the file scan's chunks go through rayon (`d download` p50
  2.29 → 2.02 ms, and the pool parks between searches: 0 CPU ticks over 90 s
  idle); a streaming `top` no longer runs on the read loop, so typing does not
  queue behind a full history build; the usage database gained an
  `(on_click, key)` index and WAL, taking a one-shot `select` from 8 fdatasyncs
  to 0; icons decode on a worker thread instead of in the frame that draws them;
  the query and session-bus work moved off the runtime's workers; and a hidden
  resident launcher drops the rows it was holding.
- The release binary is linked with full LTO rather than thin: 6,984,904 →
  6,510,872 bytes (6.8% smaller, ~2.87 MB gzipped), with startup unchanged.

### Fixed

- A `run` line reaches the shell as it was typed. `%u`, `%U`, `%f` and `%F` were
  being deleted from it, so `r date +%F` ran `date +`, and a host's `run{cmd}`
  was stripped the same way; the terminal path never did it, so which of the two
  a command took decided whether the codes survived. `jsonrpc.md` now states that
  only `launch` and `desktop_action` read `.desktop` field codes.
- The shell's two external interactions are bounded. A clipboard read is killed
  after one second and answered with no text, so a stuck selection owner cannot
  wedge a paste, and the pipe is read from a thread of its own so a selection
  larger than the pipe buffer cannot deadlock it. The IPC listener takes the
  `wayrun.lock` flock before binding the socket, closing the race where two
  simultaneous starts both unlinked it.
- The shared copy-link action files its default under its own `copy-url` scope
  instead of inheriting `firefox-bookmarks`, so a remembered default applies to
  every URL row whichever provider answered.
- Opening or closing the action panel re-derives the hover, so a stationary
  pointer no longer highlights a row that moved out from under it; the shell's
  `select` payload sends `summary` and `icon` as `null` rather than an empty
  string, matching the rows the core emits.
- A `.desktop` action's `%c` comes from the same locale list as the row's title,
  so a session that sets `LC_ALL` or `LANGUAGE` over `LANG` no longer launches an
  action named in a different language than the panel showed.

## [0.4.0] - 2026-09-24

### Added

- `f` and `d` search the whole home directory at any depth through a background
  index (`$XDG_CACHE_HOME/wayrun/file-index.bin`): built on the first `f`/`d`
  search, mapped only while in use and unmapped after two minutes idle, and
  re-checked within a minute by a parallel sweep that re-reads just the
  directories whose mtime moved — a patch of ~0.1 s against the walk of ~1.2 s
  it replaces. Every row carries a bigram bloom (index format v4) that the scans
  reject on before touching a name. `config.md`'s `[files]` holds `index`
  (default `true`), `depth` (the unindexed fallback, `3`) and `exclude`.
- `[files] exclude` names the directories the walk never enters. The shipped
  `config.toml` lists `node_modules`, `target` and `__pycache__`; a name
  starting with a dot is always hidden, and an edit to the list rebuilds on the
  next `f`/`d` search.

### Changed

- **Breaking**: a plain query no longer stops at the first provider with
  results. `calculator`, `system-commands` and `app-search` all answer, and
  their rows are merged by each row's relevance — the match kind's weight scaled
  by the surface it matched on — then by the row's usage count, then by
  registry order, so a prefix command can no longer hide an exact app.
- **Breaking**: the app list's metadata surfaces are visible in a plain query:
  a keyword or `GenericName` matches by word and a `Comment` by prefix, and a
  near spelling of a name answers too. A title match leads a description match
  of comparable strength, while an exact keyword still beats a title the query
  only sits inside of.
- Every provider ranks by one match vocabulary: the file index, the path scans,
  the runner, the window list and the app list share the same kinds, and
  `nucleo` is gone with its transitive crates. A `f`/`d` build logs itself on
  stderr (`file index … (patched)` / `(loaded)` / `(capped)`).

### Fixed

- A clipboard preview longer than 80 bytes is cut on a character boundary
  instead of panicking on a multi-byte preview, which silently emptied the
  whole list.
- A keystroke burst no longer runs one search per key: the core coalesces
  superseded queries and drops their payloads.
- Firefox snapshots are opened read-only and immutable, so a `b`/`h` query takes
  no locks on the private copy.
- The plugin host cache (now under `$XDG_CACHE_HOME/wayrun`) and the built-in
  glyphs are written atomically, so a crash mid-write cannot leave a torn file
  behind.
- The file index only serves a cache it can vouch for: an unreadable walk is
  never persisted as an empty index, a directory layout that is not the walk's
  own pre-order is rejected, a non-UTF-8 `$HOME` still indexes and serves, and
  the first search after an idle unmap remaps instead of falling back to the
  walk.
- The index's own search no longer pays for work it discards: every indexed row
  ran a `Loose` subsequence pass its caller threw away and then a second
  substring pass. `f report` on this home's 555k-name index went ~12 ms -> ~3 ms
  — confident-only classifiers, a `memchr2` candidate walk that fuses the word
  and substring checks, and a per-row bloom the scans reject on before touching
  a name.
- An index build's peak memory fell 144 MB -> 96 MB on this home: the walk keeps
  one length-prefixed names buffer per directory instead of one allocation per
  name, and the image streams into the cache file instead of being materialised
  beside the tables.

## [0.3.1] - 2026-09-22

### Added

- The interface is translated: `locales/en.yml` and `locales/zh_cn.yml` hold
  every string the launcher shows, and the locale is resolved once at startup
  from `$LC_ALL`, `$LC_MESSAGES`, `$LANG` — `zh_CN.UTF-8` becomes `zh_cn`, any
  `zh_*` variant reads the one Chinese table, anything else falls back to
  English. Plugin names and ready hints, action titles, the `?` help line, the
  system commands and the web-search rows are all covered. `wayrun status`,
  `--list-plugins` and the RPC errors stay English, and whatever a host sends
  is relayed as it is.
- `config.toml`'s `[ui] locale` pins the interface language regardless of the
  session, which is what a resident service needs: its unit inherits the
  session's locale, or none at all. It is read at startup, so a change needs a
  restart.
- The action panel leads with the row's own command (**Open**), so its first
  slot is what Enter runs: `Shift+Enter` then Enter opens the row instead of
  pinning it, and `Alt+Enter` on **Open** clears a remembered default whichever
  scope set it.
- A remembered default is visible without opening the panel: the row carries the
  same dot, its footer names the action Enter will run (`⏎ Open in terminal`)
  instead of `⏎ Launch`, and `?` lists each plugin's remembered action.
- An action an external host attaches can be made the default Enter action: the
  core stamps the host as the owner of the actions it parses out of a response,
  so the host's scope is what the gesture sets.
- A new launcher mark, `images/logo.svg` (replacing
  `images/application_default.png`), trails three fading echoes behind the W,
  and the built-in `builtin:app` glyph is drawn from the same rest geometry.

### Changed

- A file or directory row lists "Open in terminal" next to **Open**, with
  "Reveal in file manager" and "Copy path" after them, so the open-family
  entries are grouped at the top of the panel instead of the terminal being
  last.
- Pin/unpin and the default gesture re-send the query with the panel held open
  on the entry that changed, so the change lands where it was made. The resume
  is dropped by a close, a hide and every query edit, so a slow host's reply
  cannot resurrect a panel the user closed.
- Both README editions no longer name Papirus as the icon theme: app and file
  rows follow the desktop's own theme. A License section credits the bundled
  Material Symbols (Apache-2.0) and links the notice.
- The user-facing pages state what a feature is and how to use it: the
  implementation narrative (process split, caches, internal paths) is gone, and
  the two language editions keep the same sections.

## [0.3.0] - 2026-09-22

### Changed

- **Breaking**: an external plugin host must return absolute paths for every
  icon it emits — its `list_plugins` identity, result rows, their actions and a
  row's `badge`. The core no longer resolves a theme icon name, a `papirus:`
  spec or a `builtin:` glyph for a host; a missing or non-absolute icon falls
  back to the plugin identity icon, then the built-in placeholder. The
  `resolve_icon` JSON-RPC method is removed and answers `-32601`.
- Icons are resolved through the desktop's icon theme instead of a hardcoded
  Papirus scan: `system/icon.rs` reads each theme's `index.theme` (directories,
  size ranges and `Inherits`) and walks the theme chain, then `hicolor`, then
  `/usr/share/pixmaps`.
- The README demo is re-recorded over new wallpapers, showing file search and a
  Firefox bookmark search.
- The docs state the real platform scope: any compositor that implements
  `wlr-layer-shell` works (GNOME/Mutter does not), and each quick-search prefix
  lists the optional piece it needs.

### Added

- Built-in UI glyphs: `builtin:` Material Symbols compiled into the binary for
  the panel, badges and built-in plugin identities, so the launcher works with
  no icon theme installed. A miss falls back to `builtin:app`, and the core
  warns on stderr when no icon theme is found.
- A new launcher mark (the `app` glyph) and a `NOTICE` crediting the bundled
  Material Symbols.

## [0.2.0] - 2026-09-21

### Added

- Window search on Hyprland: compositor backends are a compile-time toggle
  (`compositor-niri` default, `compositor-hyprland` experimental), and a build
  can drop both so `w` simply returns nothing.
- Path queries reach the whole home tree: a `f`/`d` query containing `/` or a
  leading `~` is stat'd directly, opening `~/Project/WayRun` even past the
  depth-3 walk; an absolute path is taken wherever it points.
- File rows offer *Copy path* and *Open in terminal*, and a `$PATH` hit launches
  through its `.desktop` app (honoring `Terminal=`) or in a terminal.
- `Alt+Enter` remembers a panel action as its plugin's Enter default (the
  `default` JSON-RPC method), marked in the panel and stored in `usage.db`.
- Web suggestions honor `https_proxy` / `http_proxy` from the environment.

### Changed

- **Breaking**: the wire protocol is JSON-RPC 2.0 with typed commands only. A
  non-JSON line answers `-32700`, `search`/`top` stream a `results` notification
  without an `id` and answer synchronously with one, and `on_click`/panel actions
  are `Command`/`PanelAction` objects instead of scheme strings. See
  `docs/en/jsonrpc.md`.
- File and path results are ranked by match quality (exact name, then prefix,
  substring, parent-path) with depth as a tie-break, instead of walk order.
- Firefox history is ordered by `moz_places.last_visit_date`, so one row per
  place replaces the per-visit duplicates.
- File icons come from the system MIME database's themed-icon chain, replacing
  the hand-kept suffix table.
- Parsing a core notification is one typed pass, so the results path no longer
  materialises a `serde_json::Value` tree.
- A window search caches the compositor's list for 500ms across a typing burst,
  and a bookmark/history search reads a cached `places.sqlite` copy keyed by
  `(mtime, size)` instead of copying 30MB per keystroke.
- The plugin registry is split into `model`/`registry`/`actions`/`search`
  modules, with no behaviour change.

### Fixed

- Send query changes as JSON-RPC searches; a raw line was answered `-32700` and
  left every list empty.
- Record the selected row in usage history when its action runs from the panel,
  not only on Enter.
- Scope a remembered default action's id to its app, and reject a non-string
  `action_id` instead of treating it as a clear.
- Attach *Remove from history* only to a recordable history row and *Pin to top*
  only to a non-`ephemeral` row.
- Rank window rows by the shared name tier, so an exact `app_id` leads a title
  that merely contains the query.
- Render an SVG panel glyph crisp at its final size rather than scaling the
  cropped ink.
- An iconless row falls back to its plugin's icon, so a command or window row no
  longer shows the bundled placeholder.

## [0.1.3] - 2026-09-20

### Added

- `config.toml` (`~/.config/wayrun/config.toml`): the core's user settings, every
  key optional. `[web_search].engine` picks Google or DuckDuckGo, and
  `[font].family` sets the shaping family (a restart applies it).
- The rest of the card in `theme.toml`: `[layout]` exposes the radii, width
  ratio/bounds, top ratio, alignment and offsets, `[colors].dim` the backdrop,
  and `[font].size` the interface size.
- Per-mode colour tables: `[colors.dark]` and `[colors.light]` layer on
  `[colors]` for the mode the core reports, so a table states only what it
  changes.
- The footer is redrawn as keycap hints plus a result-count pill.
- The README demo is an animated AVIF, embedded as a plain `<img>`: smaller than
  the WebP and free of its lossy blocking.

### Changed

- `plugins.toml`'s `enable` field is now `enabled`.
- `theme.toml` colour overrides converge on the `primary` / `fg` / `container`
  roles plus per-surface colours, replacing the older per-field table.
- The clear button and return hints use `×` and `⏎`, glyphs the shaping family
  covers, so a missing one no longer triggers a full fallback-font scan.
- Dependencies shrink (`itertools` and `rustc-hash` dropped, unused features
  trimmed) and CI collapses to one job that ships a `.tar.gz` artifact.

### Fixed

- A `theme.toml` / `config.toml` reload needs no debounce, and a reload never
  rewrites the file: the shipped template is written once with `create_new`, so
  an editor's atomic save is not truncated.

## [0.1.2] - 2026-09-19

### Added

- Action panel on `Shift+Enter`: a Wox-style second level whose entries come from
  the plugin that owns the row. Files offer *Reveal in file manager*, applications
  list their `[Desktop Action …]` groups, bookmarks and search hits offer *Copy
  URL*, and every actionable row gets the launcher-level *Pin to top* / *Unpin*
  and *Remove from history*.
- Pins: a row can be stored under its exact trimmed query (`""` is the
  empty-query history) in `usage.db` and is re-emitted first, with a pin badge,
  the next time that same string is searched. Pins are scoped, so pinning on
  `b firefox` never leads `b`.
- `reveal`, `pin` and `unpin` JSON-RPC methods; result items may carry an
  `actions` array and a `badge`, and the panel-only schemes are `pin:`, `unpin:`,
  `forget:` and `reveal:`.
- An animated WebP demo in the README.

### Changed

- The release binary drops from 19MB to 7.2MB (~3.0MB gzipped). `reqwest` is
  gone: its default rustls backend statically linked the 7.4MB aws-lc crypto
  library plus hyper and h2 for a single GET. The web provider now uses `minreq`
  over the system OpenSSL, and the release profile is stripped with thin LTO, one
  codegen unit and `panic = "abort"`.
- Dismissing the resident shell releases its shaping font mappings, so the hidden
  backend falls from ~34.5MB RSS / 20.6MB Pss with 18.6MB of font pages to
  ~14.6MB / 7.8MB with none.
- `Delete` is an ordinary forward-delete in the query field; forgetting a history
  item moved into the action panel.
- Web suggestions URL-encode the query and give up after 5 seconds.

### Fixed

- Drop the translate icon left behind by the removed plugin.

## [0.1.1] - 2026-09-19

### Added

- System theme from DankMaterialShell: the launcher reads the Material You palette
  matugen generates (`~/.cache/DankMaterialShell/dms-colors.json`) and follows
  wallpaper and light/dark changes live, falling back to the built-in dark
  palette without DMS.
- `theme.toml`: an optional `~/.config/wayrun/theme.toml` overrides colors per
  field and controls the card through `[blur]`, `[appearance]`, `[layout]` and
  `[motion]`. Every key is optional, out-of-range values are clamped, and the file
  is watched so edits apply without a restart.
- Documentation split into per-topic pages under `docs/en/` and `docs/zh_cn/`.

### Changed

- Repaint only the card rectangle on a settled frame and upload only the damaged
  region, cutting the per-frame cost while the launcher is idle.

### Fixed

- Dispatch the `action:` verb so `[Desktop Action …]` rows actually run.
- Open the usage database without panicking when `$HOME` is missing or the file is
  read-only, and skip corrupt history rows instead of rejecting the list.
- Drain the core's stdout on a plain thread so a one-shot JSON-RPC client always
  gets its last reply.
- Stop walking the home subdirectories twice in file search.
- Keep a keyword search self-contained, so a miss never falls through to the
  default providers.

## [0.1.0] - 2026-09-19

The first release of WayRun: a Wayland-native application launcher and
quick-search tool. A single `wayrun` binary is both the floating overlay shell
and, with `--core`, the backend service.

### Added

- Fuzzy app launcher over `.desktop` entries, launched through the GLib
  `GAppInfo` registry (proper `Exec=` quoting and `DBusActivatable`).
- File and path search, Firefox bookmarks and history, clipboard history
  (`cliphist`), inline math, and `$PATH` commands with args.
- Switch to any open niri window, and open URLs with the default handler.
- Usage-ranked history on empty input; `Delete` removes an entry.
- GTK4 theme colours and Papirus / Breeze / Adwaita / hicolor icon resolution.
- Plugin registry in `plugins.toml`, with optional external JSON-RPC 2.0 hosts.
- Resident mode over `$XDG_RUNTIME_DIR/wayrun.sock` for zero cold-start
  (`wayrun toggle`).

[Unreleased]: https://github.com/Prslc/WayRun/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/Prslc/WayRun/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/Prslc/WayRun/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/Prslc/WayRun/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Prslc/WayRun/compare/v0.1.3...v0.2.0
[0.1.3]: https://github.com/Prslc/WayRun/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/Prslc/WayRun/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/Prslc/WayRun/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/Prslc/WayRun/releases/tag/v0.1.0
