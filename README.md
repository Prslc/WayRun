<div align="center">

# WayRun

<img src="images/logo.svg" alt="App Icon" width="150" height="150"><br>

English | [Chinese](docs/zh_cn/README_CN.md)

[![CI](https://github.com/Prslc/WayRun/actions/workflows/ci.yml/badge.svg)](https://github.com/Prslc/WayRun/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Prslc/WayRun?color=4a90d9&label=release)](https://github.com/Prslc/WayRun/releases)
[![License](https://img.shields.io/badge/license-MIT_OR_Apache--2.0-yellow)](#license)
[![Rust](https://img.shields.io/badge/rust-stable-orange?logo=rust)](https://www.rust-lang.org/)
[![Wayland](https://img.shields.io/badge/Wayland-native-4a90d9?logo=wayland&logoColor=white)](https://wayland.freedesktop.org/)

</div>

A Wayland-native application launcher and quick-search tool for Linux. Type to
search installed apps, files and inline math, all from a single floating
overlay.

## Overview

It ships as one Rust binary, `wayrun`: the launcher, plus the backend it talks to
(`--core`). The overlay is drawn in software into a shared-memory buffer, so it
needs no GPU stack and no GUI toolkit.

## Demo

<p align="center">
  <img src="images/demo.avif" alt="WayRun — app search, file search, the action panel and an image preview" width="780">
</p>

## Features

- **Launcher** — fuzzy search of installed applications, launched the way the
  desktop itself would, with Flatpak apps and themed icons per row.
- **Quick search** — files and paths, `$PATH` commands, system commands, and
  inline math.
- **Action panel** — `Shift+Enter` opens a per-type action menu (reveal a
  file, run a desktop action, copy a link).
- **Themeable** — follows the desktop's colours (KDE's colour scheme or
  DankMaterialShell's Material You palette), with `theme.toml` overrides for
  colors, blur, layout, typography and motion.
- **Extensible** — seven built-ins in the binary; everything else is an
  external host over a documented JSON-RPC contract, written in Python (a
  shipped framework) or as one Lua script the launcher runs itself. A
  `resident` host stays warm at about a millisecond per call.

## Requirements

- A **Wayland** compositor that implements `wlr-layer-shell`
  (`zwlr_layer_shell_v1`): the wlroots family (niri, Hyprland, Sway, river,
  Wayfire, labwc, ...), KDE Plasma on Wayland, and Mir-based compositors.
  GNOME/Mutter does not implement it, so the overlay cannot be shown there.
- A Rust toolchain and the usual system libraries (GLib, SQLite, xkbcommon,
  fontconfig, ...); `cargo build --release` builds everything.
- A font with CJK coverage for Chinese/Japanese queries (e.g. Source Han Sans).

## Feature dependencies

The overlay, and the built-in app, file and calculator search, need
nothing beyond the compositor above. Every other feature depends on an optional
component; when one is missing, the keyword returns no rows rather than an
error. Extra plugins (Firefox bookmarks and history, web suggestions, clipboard
history, and more) install from the
[WayRun-Plugins](https://github.com/Prslc/WayRun-Plugins) workspace:

| Feature | Needs |
| --- | --- |
| copy / paste and the "Copy …" actions | `wl-clipboard` (`wl-copy` / `wl-paste`) |
| a terminal-only `$PATH` hit (`r`) | a terminal emulator (`$TERMINAL`, else one on `PATH`) |
| a blurred card | `ext-background-effect-v1` (niri) or the compositor's own blur by namespace |
| per-row icons | a desktop **icon theme** (optional); without one, rows fall back to a bundled placeholder |

The action panel and built-in plugin glyphs are compiled into the binary,
so the launcher is fully usable with no icon theme at all. App and file rows
follow the desktop's icon theme; see [config.md](docs/en/config.md) for the
`[icon]` override.

## Plugins

The built-ins in the table above ship inside the binary. Anything more is a
host: an executable that speaks the documented JSON-RPC subset, registered from
`plugins.toml`, forked per call or kept warm with `resident = true`. The
[WayRun-Plugins](https://github.com/Prslc/WayRun-Plugins) workspace supplies a
Python framework, Lua and Python examples (`firefox.lua`, `web.lua`, …) and
templates to copy from. A Lua plugin is one script file — the launcher binary
runs it itself, so there is no interpreter to install; see
[Plugins](docs/en/plugins.md) and [Lua plugins](docs/en/lua.md).

## Quick Start

```bash
git clone https://github.com/Prslc/WayRun.git
cd WayRun
cargo build --release
ln -s "$(pwd)/target/release/wayrun" ~/.local/bin/wayrun
```

The published binaries are built with fat LTO and one codegen unit; a local
build uses thin LTO and parallel codegen, so it compiles faster and comes out
slightly larger.

Bind a hotkey (e.g. Alt+Space) to launch the shell — the syntax is
compositor-specific (niri and Hyprland examples are in
[resident mode](docs/en/resident.md)):

```bash
wayrun run
```

The launcher is a full-screen overlay with a dimmed backdrop and a centered card.
In the default spawn-per-hotkey flow, `Esc` / clicking outside quits it; in
resident mode the hotkey toggles the surface and dismiss hides it.

The same binary carries its own controls:

```bash
wayrun                                  # list the commands in a terminal
wayrun toggle                           # resident mode: show or hide over the socket
wayrun config set font.family "Noto Sans"
wayrun plugin add ~/plugins/firefox.lua --keyword ff
```

## Documentation

- [Usage](docs/en/usage.md) — prefixes, keybindings, and copy/paste.
- [Resident mode](docs/en/resident.md) — the systemd unit and the control commands.
- [Theme](docs/en/theme.md) — the system palette and `theme.toml` (colors, blur,
  layout, typography, motion).
- [Config](docs/en/config.md) — `config.toml` (search engine, font family).
- [Plugins](docs/en/plugins.md) — `plugins.toml`, the built-ins, and external
  JSON-RPC hosts.
- [Lua plugins](docs/en/lua.md) — one script the launcher hosts itself: the
  `wayrun` table, the sandbox, and registering it.
- [JSON-RPC 2.0](docs/en/jsonrpc.md) — the wire protocol and the result schema.

## Credit

- **[Wox](https://github.com/wox-launcher/wox)** — the launcher this is inspired by.
- **[tiny-skia](https://github.com/RazrFalcon/tiny-skia)** — software rasterisation for the overlay.
- **[cosmic-text](https://github.com/pop-os/cosmic-text)** — text shaping for the overlay.
- **[smithay-client-toolkit](https://github.com/Smithay/client-toolkit)** — Wayland layer-shell client plumbing.

## License

Dual licensed under either of the following, at your option:

- **MIT** — [LICENSE-MIT](LICENSE-MIT)
- **Apache-2.0** — [LICENSE-APACHE](LICENSE-APACHE)

The built-in UI glyphs are Google Material Symbols under the Apache License 2.0;
see [ui/assets/icons/NOTICE](ui/assets/icons/NOTICE) for the attribution and
the full license text.
