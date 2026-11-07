# 安装

WayRun 只有一个 Rust 可执行文件。下载发布版或自行构建，放进 `PATH`，再绑一个热键即可。

## 环境要求

- **Wayland** 合成器，需实现 `wlr-layer-shell`（`zwlr_layer_shell_v1`）。包括 wlroots 系（niri、Hyprland、Sway、river、Wayfire、labwc 等）、KDE Plasma 的 Wayland 会话，以及基于 Mir 的合成器。GNOME/Mutter 未实现该协议，无法显示覆盖层。
- 它链接的运行时库：GLib、SQLite、xkbcommon、fontconfig 等。
- 从源码构建时，还需要 Rust 工具链与这些库的开发头文件。
- 一款包含中日韩（CJK）字形的字体，用于中日韩查询，如 Source Han Sans。

## 下载发布版

发布版提供预编译的 x86_64 二进制，在 Ubuntu 24.04 上构建。从 [GitHub Releases](https://github.com/Prslc/WayRun/releases/latest) 下载最新的 `wayrun-v*-x86_64-unknown-linux-gnu.tar.gz`，然后：

```bash
tar -xzf wayrun-v*-x86_64-unknown-linux-gnu.tar.gz
install -Dm755 wayrun ~/.local/bin/wayrun
```

`~/.local/bin` 需要在 `PATH` 中。压缩包同时带有内置图标的 `NOTICE`。其他架构或更旧的发行版请从源码构建。

## 从源码构建

```bash
git clone https://github.com/Prslc/WayRun.git
cd WayRun
cargo build --release
ln -s "$(pwd)/target/release/wayrun" ~/.local/bin/wayrun
```

发布版以 fat LTO 和单个 codegen unit 构建；本地构建保留默认设置，编译更快，体积略大。

## 首次启动

给一个热键（例如 Alt+Space）绑定 `wayrun run`：

```bash
wayrun run
```

热键的具体写法取决于合成器。niri 与 Hyprland 的示例见[常驻模式](resident.md)。

首次运行会生成带注释的 `config.toml`、`theme.toml` 与 `plugins.toml` 模板。键的含义见[配置](config.md)、[主题](theme.md)与[插件](plugins.md)。

## 功能依赖

覆盖层本身，以及内置的应用、文件与计算搜索，只需要一个支持上述协议的合成器。其余功能各自还依赖一个可选组件。缺少组件时，对应关键词返回空结果，不会报错。

更多插件（Firefox 书签与历史、网页建议、剪贴板历史等）从 [WayRun-Plugins](https://github.com/Prslc/WayRun-Plugins) 工作区安装。下表列出各项功能所需的组件：

| 功能 | 依赖 |
| --- | --- |
| 复制与粘贴，以及「复制 …」动作 | `wl-clipboard`（`wl-copy` / `wl-paste`） |
| 终端中打开的 `$PATH` 命中项（`r`） | 一个终端模拟器（`$TERMINAL`，否则取 `PATH` 中的一个） |
| 模糊卡片 | `ext-background-effect-v1`（niri、KWin），或合成器按命名空间提供的模糊 |
| 逐行图标 | 可选的桌面**图标主题**；没有时回退到内置占位图标 |
