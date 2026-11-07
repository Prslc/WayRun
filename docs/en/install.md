# Installation

WayRun is one Rust binary. Download a release or build it, put it on `PATH`,
and bind a hotkey.

## Requirements

- A **Wayland** compositor that implements `wlr-layer-shell`
  (`zwlr_layer_shell_v1`). This covers the wlroots family (niri, Hyprland,
  Sway, river, Wayfire, labwc, ...), KDE Plasma on Wayland and Mir-based
  compositors. GNOME/Mutter does not implement the protocol, so the overlay
  cannot be shown there.
- The runtime libraries it links against: GLib, SQLite, xkbcommon, fontconfig
  and the rest.
- A Rust toolchain and the development headers for those libraries, if you
  build from source.
- A font with CJK coverage for Chinese and Japanese queries, such as Source Han
  Sans.

## Download a release

Releases ship a prebuilt binary for x86_64, built on Ubuntu 24.04. Download the
latest `wayrun-v*-x86_64-unknown-linux-gnu.tar.gz` from
[GitHub Releases](https://github.com/Prslc/WayRun/releases/latest), then:

```bash
tar -xzf wayrun-v*-x86_64-unknown-linux-gnu.tar.gz
install -Dm755 wayrun ~/.local/bin/wayrun
```

`~/.local/bin` must be on your `PATH`. The tarball also carries `NOTICE` for
the bundled icons. On another architecture or an older distribution, build
from source.

## Build from source

```bash
git clone https://github.com/Prslc/WayRun.git
cd WayRun
cargo build --release
ln -s "$(pwd)/target/release/wayrun" ~/.local/bin/wayrun
```

Published binaries use fat LTO and one codegen unit; a local build keeps the
defaults, compiles faster, and comes out slightly larger.

## First launch

Bind a hotkey, for example Alt+Space, to `wayrun run`:

```bash
wayrun run
```

The syntax for binding a key is compositor-specific. niri and Hyprland
examples are in [resident mode](resident.md).

The first run writes commented templates for `config.toml`, `theme.toml` and
`plugins.toml`. See [Config](config.md), [Theme](theme.md) and
[Plugins](plugins.md) for their keys.

## Feature dependencies

The overlay, and the built-in app, file and calculator search, need nothing
beyond the compositor above. Every other feature depends on an optional
component. When one is missing, the keyword returns no rows rather than an
error.

Extra plugins (Firefox bookmarks and history, web suggestions, clipboard
history, and more) install from the
[WayRun-Plugins](https://github.com/Prslc/WayRun-Plugins) workspace. The table
lists what each feature needs:

| Feature | Needs |
| --- | --- |
| copy / paste and the "Copy …" actions | `wl-clipboard` (`wl-copy` / `wl-paste`) |
| a terminal-only `$PATH` hit (`r`) | a terminal emulator (`$TERMINAL`, else one on `PATH`) |
| a blurred card | `ext-background-effect-v1` (niri, KWin) or the compositor's own blur by namespace |
| per-row icons | a desktop **icon theme** (optional); without one, rows fall back to a bundled placeholder |
