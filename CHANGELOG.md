# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/Prslc/WayRun/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/Prslc/WayRun/compare/v0.1.3...v0.2.0
[0.1.3]: https://github.com/Prslc/WayRun/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/Prslc/WayRun/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/Prslc/WayRun/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/Prslc/WayRun/releases/tag/v0.1.0
