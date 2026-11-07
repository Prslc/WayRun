# Installation

WayRun is one Rust binary. Build it, put it on `PATH`, and bind a hotkey.

## Requirements

- A **Wayland** compositor that implements `wlr-layer-shell`
  (`zwlr_layer_shell_v1`). This covers the wlroots family (niri, Hyprland,
  Sway, river, Wayfire, labwc, ...), KDE Plasma on Wayland and Mir-based
  compositors. GNOME/Mutter does not implement the protocol, so the overlay
  cannot be shown there.
- A Rust toolchain and the usual system libraries (GLib, SQLite, xkbcommon,
  fontconfig, ...). `cargo build --release` builds everything.
- A font with CJK coverage for Chinese and Japanese queries, such as Source Han
  Sans.

## Build and install

```bash
git clone https://github.com/Prslc/WayRun.git
cd WayRun
cargo build --release
ln -s "$(pwd)/target/release/wayrun" ~/.local/bin/wayrun
```

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
