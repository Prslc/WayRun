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
  <img src="images/demo.avif" alt="WayRun — app search, file search and dynamic theming" width="780">
</p>

## Features

- **Launcher** — fuzzy search of installed applications, launched the way the
  desktop itself would, with Flatpak apps and themed icons per row.
- **Quick search** — files and paths, clipboard history, `$PATH` commands,
  open windows, system commands, and inline math.
- **Action panel** — `Shift+Enter` opens a per-type action menu (reveal a
  file, run a desktop action, copy a link).
- **Themeable** — follows DankMaterialShell's Material You palette, with
  `theme.toml` overrides for colors, blur, layout, typography and motion.
- **Extensible** — eight built-ins in the binary; everything else is an
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
error. Extra plugins (Firefox bookmarks and history, web suggestions, and more)
install from the [WayRun-Plugins](https://github.com/Prslc/WayRun-Plugins)
workspace:

| Feature | Needs |
| --- | --- |
| `w` open-window search | the niri (default) or Hyprland build — see [Compositor backends](#compositor-backends) |
| `c` clipboard history | [cliphist](https://github.com/sentriz/cliphist) running |
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
wayrun
```

The launcher is a full-screen overlay with a dimmed backdrop and a centered card.
In the default spawn-per-hotkey flow, `Esc` / clicking outside quits it; in
resident mode the hotkey toggles the surface and dismiss hides it.

## Compositor backends

The overlay works on any supported compositor; only open-window search (`w`)
needs a per-compositor build, selected by cargo features: `compositor-niri`
(the default) and `compositor-hyprland` (experimental). Build just the one you
run; with no backend compiled the launcher still runs and `w` returns nothing.

```bash
cargo build --release                                            # niri
cargo build --release -p wayrun-shell --no-default-features --features compositor-hyprland
cargo build --release -p wayrun-shell --no-default-features      # no window search
```

## Performance

A resident core answers a warm query over its JSON-RPC pipe in 0.2–2 ms,
payloads included:

| Query | Rows | P50 | P95 |
|---|---|---|---|
| app search — `firefox` | 1 | 0.18 ms | 0.29 ms |
| file search — `f report` | 50 | 1.88 ms | 2.03 ms |
| directory search — `d download` | 50 | 1.10 ms | 1.31 ms |
| path lookup — `~/` | 15 | 0.50 ms | 0.61 ms |

Cold start: the first `ping` reply comes back in 10.0 ms, and a cold app
search takes 24 ms.

A warm panel show — every show after the process's first — commits its first
frame in 3.6 ms (P95 4.8 ms); that first show takes ~10 ms. The first frame is
transparent (the entrance starts at alpha 0), so the first visible frame lands
one refresh later, and the 240 ms fade is deliberate.

<details>
<summary>Full table, cold start and method</summary>

Warm steady state (3 discarded + 30 measured rounds per run, shuffled):

| Provider | Query | Rows | P50 (ms) | P95 (ms) | Max (ms) |
|---|---|---|---|---|---|
| `ping` | — | — | 0.042 | 0.060 | 0.123 |
| `calculator` | `2 + 3 * 4` | 1 | 0.163 | 0.290 | 0.334 |
| `system-commands` | `reboot` | 1 | 0.189 | 0.258 | 0.276 |
| `app-search` | `firefox` | 1 | 0.180 | 0.290 | 0.325 |
| `runner` | `r htop` | 10 | 0.383 | 0.583 | 0.638 |
| `file-search` | `f report` | 50 | 1.876 | 2.029 | 2.598 |
| `path-search` | `d download` | 50 | 1.104 | 1.312 | 1.400 |
| `path-lookup` | `~/` | 15 | 0.500 | 0.609 | 0.689 |
| `home-fallback` | `Proj` | 1 | 0.263 | 0.345 | 0.444 |
| `clipboard` | `c a` | 50 | 0.191 | 0.218 | 0.235 |
| `window` | `w kitty` | 1 | 0.085 | 0.110 | 0.114 |

Cold first call (a fresh `wayrun --core` per run, 3 runs, run 1 shown):

| Provider | Query | Rows | Index | Wall (ms) | MaxRSS (MB) |
|---|---|---|---|---|---|
| `ping` | — | — | — | 10.0 | 10.4 |
| `calculator` | `2 + 3 * 4` | 1 | — | 23.3 | 14.2 |
| `system-commands` | `reboot` | 1 | — | 23.1 | 14.8 |
| `app-search` | `firefox` | 1 | — | 23.6 | 14.8 |
| `runner` | `r htop` | 10 | — | 33.6 | 15.2 |
| `file-search` | `f report` | 50 | loaded | 57.5 | 52.8 |
| `path-search` | `d download` | 50 | loaded | 48.7 | 40.1 |
| `path-lookup` | `~/` | 15 | — | 23.7 | 14.7 |
| `home-fallback` | `Proj` | 1 | — | 22.4 | 14.2 |
| `clipboard` | `c a` | 50 | — | 33.8 | 13.2 |
| `window` | `w kitty` | 1 | — | 24.0 | 16.5 |

Show latency (IPC open → first committed frame, 3 warmups + 20 shows):

| | Cold | Warm P50 | Warm P95 |
|---|---|---|---|
| show | 10.1 ms | 3.6 ms | 4.8 ms |

Measured 2026-10-18 on an AMD Ryzen 5 5500U (6c/12t, `performance` governor),
Arch Linux, niri 26.04, release build. Warm rows are synchronous `search`
calls against one resident `--core` over stdin/stdout; cold rows start a fresh
process each run, timed with GNU time; show rows measure a resident shell's
`toggle` to its first committed frame (`WAYRUN_TIMING`). The real `$HOME` is
read, with a 42 MB file index over 111k directories and 742k files. `ping` is
the harness's own round-trip floor, and the clipboard and window rows hit a
500 ms burst cache.

</details>

## Documentation

- [Usage](docs/en/usage.md) — prefixes, keybindings, and the clipboard.
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
see [shell/assets/icons/NOTICE](shell/assets/icons/NOTICE) for the attribution and
the full license text.
