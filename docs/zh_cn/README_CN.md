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

一款 Wayland 原生的 Linux 应用启动器与快速搜索工具。在悬浮窗口中输入关键词，即可搜索已安装应用与文件，并进行即时数学计算。

## 概述

整个项目只产出一个 Rust 可执行文件 `wayrun`：启动器本体，以及它通过 `--core` 使用的后端。覆盖层完全用软件光栅化进共享内存缓冲，因此不涉及 GPU 栈，也不依赖任何 GUI 工具包。

## 演示

<p align="center">
  <img src="../../images/demo.avif" alt="WayRun —— 应用搜索、文件搜索、动作面板与图片预览" width="780">
</p>

## 功能特性

- **应用启动**：模糊搜索已安装应用，按桌面自身的方式启动，并逐行显示 Flatpak 应用与主题图标。
- **快速搜索**：文件与路径、`$PATH` 命令、系统命令，以及即时计算。
- **动作面板**：`Shift+Enter` 打开按类型区分的动作面板（定位文件、运行 Desktop Action、复制链接）。
- **可主题化**：跟随桌面的配色（KDE 配色方案或 DankMaterialShell 的 Material You 调色板），并可用 `theme.toml` 覆盖配色、模糊、布局、字体与动效。
- **可扩展**：二进制内置 7 个插件，其余都通过有文档的 JSON-RPC 契约作为外部宿主接入。可以用 Python（自带框架）写，也可以就是一个由启动器本体运行的 Lua 脚本。`resident` 宿主会跨调用保留进程，每次调用约 1 毫秒。示例与模板见 [WayRun-Plugins](https://github.com/Prslc/WayRun-Plugins) 工作区。

## 环境要求

WayRun 需要支持 `wlr-layer-shell` 的 Wayland 合成器：wlroots 系（niri、Hyprland、Sway、river、Wayfire、labwc 等）、KDE Plasma 的 Wayland 会话，或基于 Mir 的合成器。GNOME/Mutter 不支持该协议。

其余内容，包括运行时库和部分功能所需的可选组件，见[安装](install.md)。

## 快速开始

从 [GitHub Releases](https://github.com/Prslc/WayRun/releases/latest) 下载最新发布版，然后：

```bash
tar -xzf wayrun-v*-x86_64-unknown-linux-gnu.tar.gz
install -Dm755 wayrun ~/.local/bin/wayrun
```

`~/.local/bin` 需要在 `PATH` 中。压缩包同时带有内置图标的 `NOTICE`。发布版是在 Ubuntu 24.04 上构建的 x86_64 二进制；其他架构或更旧的发行版请从源码构建：

```bash
git clone https://github.com/Prslc/WayRun.git
cd WayRun
cargo build --release
ln -s "$(pwd)/target/release/wayrun" ~/.local/bin/wayrun
```

在合成器配置中给 `wayrun run` 绑定一个快捷键。启动器以浮动覆盖层打开；按键见[使用说明](usage.md)，systemd 单元见[常驻模式](resident.md)。

## 文档

同样的页面也发布为站点：<https://prslc.github.io/WayRun/zh_cn/>。

- [安装](install.md)：环境要求、下载或构建、首次启动、功能依赖。
- [使用说明](usage.md)：前缀、按键与复制粘贴。
- [配置](config.md)：`config.toml`（界面语言、字体、图标、文件索引）。
- [主题](theme.md)：系统调色板与 `theme.toml`（配色、模糊、布局、字体、动效）。
- [常驻模式](resident.md)：systemd 单元与控制命令。
- [插件](plugins.md)：`plugins.toml`、内置插件与外部 JSON-RPC 宿主。
- [插件目录](plugins-directory.md)：WayRun-Plugins 工作区里的 Lua 与 Python 示例。
- [Lua 插件](lua.md)：由启动器自身承载的一个脚本：`wayrun` API、沙箱与注册。
- [JSON-RPC 2.0](jsonrpc.md)：通信协议与结果项 schema。

## 致谢

- **[Wox](https://github.com/wox-launcher/wox)**：本启动器的灵感来源。
- **[tiny-skia](https://github.com/RazrFalcon/tiny-skia)**：覆盖层的软件光栅化。
- **[cosmic-text](https://github.com/pop-os/cosmic-text)**：覆盖层的文本整形。
- **[smithay-client-toolkit](https://github.com/Smithay/client-toolkit)**：Wayland layer-shell 客户端管线。

## 许可证

WayRun 采用双许可证，可任选其一：

- **MIT**：[LICENSE-MIT](../../LICENSE-MIT)
- **Apache-2.0**：[LICENSE-APACHE](../../LICENSE-APACHE)

内置 UI 字形为 Google Material Symbols，采用 Apache License 2.0；署名与许可证全文见 [ui/assets/icons/NOTICE](../../ui/assets/icons/NOTICE)。
