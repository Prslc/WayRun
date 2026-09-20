<div align="center">

# WayRun

<img src="images/application_default.png" alt="App Icon" width="150" height="150"><br>

English | [Chinese](docs/zh_cn/README_CN.md)

[![CI](https://github.com/Prslc/WayRun/actions/workflows/ci.yml/badge.svg)](https://github.com/Prslc/WayRun/actions/workflows/ci.yml)
[![License](https://img.shields.io/github/license/Prslc/WayRun?color=yellow)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable-orange?logo=rust)](https://www.rust-lang.org/)
[![Wayland](https://img.shields.io/badge/Wayland-native-4a90d9?logo=wayland&logoColor=white)](https://wayland.freedesktop.org/)

</div>

## Overview

WayRun is a Wayland-native application launcher and quick-search tool for Linux.
Type to search installed apps, Firefox bookmarks, web suggestions, and
inline math — all from a single floating overlay. It ships as one Rust binary,
`wayrun`, which runs as the overlay shell or, with `--core`, as the backend
service. The shell owns a `wlr-layer-shell` surface and rasterises its own card,
list and animations with
[tiny-skia](https://github.com/RazrFalcon/tiny-skia) into a `wl_shm` buffer, so
the frontend needs no GPU stack and no GUI toolkit; the core owns the plugin
registry, the JSON-RPC protocol and the usage database.

## Demo

<p align="center">
  <img src="images/demo.avif" alt="WayRun — app search, file search, dynamic theming, and bookmarks" width="780">
</p>

## Features

- **Launcher** — fuzzy `.desktop` search across XDG data dirs, launched through
  the GLib `GAppInfo` registry (Exec quoting, field codes, `DBusActivatable`),
  with Flatpak and themed icons resolved per row.
- **Quick search** — files and paths, Firefox bookmarks and history, clipboard
  history, web suggestions, `$PATH` commands, open niri windows, system commands,
  and inline math.
- **Usage history** — most-used items on an empty query.
- **Action panel & pins** — `Shift+Enter` opens a per-type action menu (reveal a
  file, run a desktop action, copy a link); `Pin to top` keeps a result first
  under its keyword.
- **Themeable** — follows DankMaterialShell's Material You palette, with
  `theme.toml` overrides for colors, blur, layout, typography and motion.
- **Extensible** — a TOML plugin registry plus external JSON-RPC hosts.

## Requirements

- **Wayland** compositor with `wlr-layer-shell` support
- Rust toolchain (`cargo build --release` builds the whole workspace; the shell is plain tiny-skia/cosmic-text crates)
- A font with CJK coverage for Chinese/Japanese queries (e.g. Source Han Sans)
- Firefox (optional, for bookmarks / history)
- [cliphist](https://github.com/sentriz/cliphist) (optional, for clipboard history)

## Quick Start

```bash
git clone https://github.com/Prslc/WayRun.git
cd WayRun
cargo build --release
ln -s "$(pwd)/target/release/wayrun" ~/.local/bin/wayrun
```

Bind a hotkey (e.g. Alt+Space) to launch the shell:

```bash
wayrun
```

The launcher is a full-screen overlay with a dimmed backdrop and a centered card.
In the default spawn-per-hotkey flow, `Esc` / clicking outside quits it; in
resident mode the hotkey toggles the surface and dismiss hides it.

## Documentation

- [Usage](docs/en/usage.md) — prefixes, keybindings, and the clipboard.
- [Resident mode](docs/en/resident.md) — the systemd unit and the IPC commands.
- [Theme](docs/en/theme.md) — the system palette and `theme.toml` (colors, blur,
  layout, typography, motion).
- [Config](docs/en/config.md) — `config.toml` (search engine, font family).
- [Plugins](docs/en/plugins.md) — `plugins.toml`, the built-ins, and external
  JSON-RPC hosts.
- [JSON-RPC 2.0](docs/en/jsonrpc.md) — the wire protocol and the result schema.

## Credit

- **[Wox](https://github.com/wox-launcher/wox)** — the launcher this is inspired by.
- **[tiny-skia](https://github.com/RazrFalcon/tiny-skia)** — software rasterisation for the overlay.
- **[cosmic-text](https://github.com/pop-os/cosmic-text)** — text shaping for the overlay.
- **[smithay-client-toolkit](https://github.com/Smithay/client-toolkit)** — Wayland layer-shell client plumbing.
- **[Papirus](https://github.com/PapirusDevelopmentTeam/papirus-icon-theme)** — the icon theme.
