<div align="center">

# WayRun

<img src="../../images/logo.svg" alt="App Icon" width="150" height="150"><br>

中文 | [English](../../README.md)

[![CI](https://github.com/Prslc/WayRun/actions/workflows/ci.yml/badge.svg)](https://github.com/Prslc/WayRun/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Prslc/WayRun?color=4a90d9&label=release)](https://github.com/Prslc/WayRun/releases)
[![License](https://img.shields.io/badge/license-MIT_OR_Apache--2.0-yellow)](#许可证)
[![Rust](https://img.shields.io/badge/rust-stable-orange?logo=rust)](https://www.rust-lang.org/)
[![Wayland](https://img.shields.io/badge/Wayland-native-4a90d9?logo=wayland&logoColor=white)](https://wayland.freedesktop.org/)

</div>

一款 Wayland 原生的 Linux 应用启动器与快速搜索工具。在悬浮窗口中输入关键词，即可搜索
已安装应用、文件、Firefox 书签、网页建议，并进行即时数学计算。

## 概述

整个项目只产出一个 Rust 可执行文件 `wayrun`：启动器本体，以及它通过 `--core` 使用的
后端。覆盖层完全用软件光栅化进共享内存缓冲，因此不涉及 GPU 栈，也不依赖任何 GUI 工具包。

## 演示

<p align="center">
  <img src="../../images/demo.avif" alt="WayRun —— 应用搜索、文件搜索、动态主题与书签" width="780">
</p>

## 功能特性

- **应用启动** — 模糊搜索已安装应用，按桌面自身的方式启动，并逐行显示 Flatpak 应用与
  主题图标。
- **快速搜索** — 文件与路径、Firefox 书签与历史、剪贴板历史、网页建议、`$PATH`
  命令、打开的窗口、系统命令，以及即时计算。
- **使用历史** — 留空时展示高频项。
- **二级菜单与置顶** — `Shift+Enter` 打开按类型区分的动作菜单（定位文件、运行
  Desktop Action、复制链接）；“置顶”让结果在其关键词下始终排在前面。
- **可主题化** — 跟随 DankMaterialShell 的 Material You 调色板，并可用
  `theme.toml` 覆盖配色、模糊、布局、字体与动效。
- **可扩展** — TOML 插件注册表，以及外部 JSON-RPC 主机。

## 环境要求

- **Wayland** 合成器，需实现 `wlr-layer-shell`（`zwlr_layer_shell_v1`）。包括
  wlroots 系（niri、Hyprland、Sway、river、Wayfire、labwc 等）、KDE Plasma 的
  Wayland 会话，以及基于 Mir 的合成器；GNOME/Mutter 未实现该协议，无法显示覆盖层。
- Rust 工具链与常见系统库（GLib、SQLite、xkbcommon、fontconfig 等）；
  `cargo build --release` 会构建全部内容。
- 覆盖中日文输入的字体（如 Source Han Sans）。

## 功能依赖

覆盖层本身，以及内置的应用、文件、计算与网页搜索，只需一个支持上述协议的合成器。其余
功能各自还依赖一个可选组件；缺少时，对应前缀只会返回空结果，而不会报错：

| 功能 | 依赖 |
| --- | --- |
| `w` 打开窗口搜索 | 编译时选择 niri（默认）或 Hyprland 后端，见[合成器后端](#合成器后端) |
| `b` / `h` Firefox 搜索 | 已配置 profile 的 Firefox |
| `c` 剪贴板历史 | [cliphist](https://github.com/sentriz/cliphist) 正在运行 |
| 复制/粘贴与“复制…”动作 | `wl-clipboard`（`wl-copy` / `wl-paste`） |
| 仅能在终端中运行的 `$PATH` 结果（`r`） | 终端模拟器（`$TERMINAL`，否则取 `PATH` 上的一个） |
| 卡片模糊 | `ext-background-effect-v1`（niri），或由合成器按 namespace 自行模糊 |
| 结果行图标 | 桌面**图标主题**（可选）；没有时回退到内置占位图 |

面板、徽标与内置插件的图形都已编入二进制，因此完全不装图标主题也能正常使用。应用与文件
结果行图标跟随桌面自身的图标主题；可用 `[icon]` 覆盖，见 [config.md](config.md)。

## 快速开始

```bash
git clone https://github.com/Prslc/WayRun.git
cd WayRun
cargo build --release
ln -s "$(pwd)/target/release/wayrun" ~/.local/bin/wayrun
```

在合成器配置中绑定一个快捷键（如 `Alt+Space`）来启动即可；绑定写法因合成器而异，
niri 与 Hyprland 的示例见[常驻模式](resident.md)：

```bash
wayrun
```

启动器以全屏覆盖方式打开，带调暗背景与居中卡片。默认的按热键拉起流程下，
`Esc`/点击卡片外会退出；常驻模式下热键切换窗口，关闭改为隐藏。

## 合成器后端

覆盖层与合成器无关；只有窗口搜索（`w`）需要按合成器单独编译，由 cargo feature 选择
后端：`compositor-niri`（默认）与 `compositor-hyprland`（实验性）。只编译你要用的
那一个；一个后端都不编译时，启动器照常运行，只是 `w` 没有结果。

```bash
cargo build --release                                            # niri
cargo build --release -p wayrun-shell --no-default-features --features compositor-hyprland
cargo build --release -p wayrun-shell --no-default-features      # 不做窗口搜索
```

## 文档

- [使用说明](usage.md) —— 前缀、按键与剪贴板。
- [常驻模式](resident.md) —— systemd 单元与控制命令。
- [主题](theme.md) —— 系统调色板与 `theme.toml`（配色、模糊、布局、字体、动效）。
- [配置](config.md) —— `config.toml`（搜索引擎、字体族）。
- [插件](plugins.md) —— `plugins.toml`、内置插件与外部 JSON-RPC 主机。
- [Lua 插件](lua.md) —— 用单个 Lua 脚本编写插件。
- [JSON-RPC 2.0](jsonrpc.md) —— 通信协议与结果项 schema。

## 致谢

- **[Wox](https://github.com/wox-launcher/wox)** —— 本启动器的灵感来源。
- **[tiny-skia](https://github.com/RazrFalcon/tiny-skia)** —— 覆盖层的软件光栅化。
- **[cosmic-text](https://github.com/pop-os/cosmic-text)** —— 覆盖层的文本整形。
- **[smithay-client-toolkit](https://github.com/Smithay/client-toolkit)** —— Wayland layer-shell 客户端管线。

## 许可证

WayRun 采用双许可证，可任选其一：

- **MIT** —— [LICENSE-MIT](../../LICENSE-MIT)
- **Apache-2.0** —— [LICENSE-APACHE](../../LICENSE-APACHE)

内置 UI 字形为 Google Material Symbols，采用 Apache License 2.0；
署名与许可证全文见 [core/assets/icons/NOTICE](../../core/assets/icons/NOTICE)。
