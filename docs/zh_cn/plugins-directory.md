# 插件目录

下面的示例都在 [WayRun-Plugins](https://github.com/Prslc/WayRun-Plugins) 工作区。它们只是参考材料：只有 `plugins.toml` 指向某个插件时，它才会被注册。多数 Lua 示例是单个脚本；`kde.lua` 与 `windows.lua` 还分别带有 `kde/` 和 `windows/` 模块。Python 示例是一个目录，自带一份 `wayrun_plugin/`。

## Lua 示例

| 插件 | 作用 | 依赖 |
| --- | --- | --- |
| [Firefox](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/firefox.lua) | 从 `places.sqlite` 搜索书签与历史 | Firefox 配置目录；`assets/` |
| [网页搜索](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/web.lua) | 搜索引擎建议 | 网络；`assets/` |
| [剪贴板](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/cliphist.lua) | 搜索 `cliphist`，把条目复制回来 | `cliphist`；`wl-clipboard` |
| [截图](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/screenshots.lua) | 最近的截图，带图片预览 | `~/Pictures/Screenshots`，或修改 `DIR` 与 `read` |
| [笔记](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/documents.lua) | 文本与 Markdown 笔记，带预览 | `documents/` |
| [GitHub](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/github.lua) | 搜索仓库与用户 | 网络；可选的 `GITHUB_TOKEN` |
| [壁纸](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/wallpaper_dms.lua) | 切换 DMS 壁纸 | `dms` 在 `PATH` 中；`~/Pictures/Wallpapers` |
| [网页直达](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/web-open.lua) | 作为默认提供者，打开输入的网址或域名 | — |
| [Man 手册](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/man.lua) | 搜索 man 手册并在终端打开 | `apropos`（man-db） |
| [汇率换算](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/currency.lua) | 按每日汇率换算货币 | 网络 |
| [KDE 设置](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/kde.lua) | 搜索 KDE 系统设置 | KDE Plasma；`systemsettings` 或 `kcmshell6` |
| [软件包](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/packages.lua) | 搜索 pacman 仓库与 AUR | Arch；`pacman`；可选的 `paru` 或 `yay` |
| [窗口切换](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/windows.lua) | 在打开的窗口之间切换 | KDE、niri 或 Hyprland；`busctl`、`niri` 或 `hyprctl` |

`firefox.lua` 与 `web.lua` 读取脚本旁的 `assets/` 目录；`documents.lua` 读取 `documents/`，`kde.lua` 读取 `kde/`，`windows.lua` 读取 `windows/`。复制脚本时把目录一起带上。

## Python 示例

| 插件 | 作用 | 依赖 |
| --- | --- | --- |
| [GitHub](https://github.com/Prslc/WayRun-Plugins/tree/main/examples/python/github) | 搜索仓库 | Python 3；自带框架 |
| [Todo](https://github.com/Prslc/WayRun-Plugins/tree/main/examples/python/todo) | 带面板动作的待办管理器 | Python 3；自带框架 |

## 安装示例

Lua 插件：克隆工作区并注册脚本。`plugin add` 会拒绝没有执行位的脚本，关键词由你决定：

```bash
git clone https://github.com/Prslc/WayRun-Plugins.git
chmod +x WayRun-Plugins/examples/lua/firefox.lua
wayrun plugin add WayRun-Plugins/examples/lua/firefox.lua --keyword ff
```

Python 插件：自己写 `plugins.toml` 条目；`plugin add` 只注册 Lua 脚本。示例自带 `wayrun_plugin/`，因此 `command` 可以直接指向 `main.py`：

```toml
[[plugins]]
id = "github"
keyword = "gh"
command = "/absolute/path/to/examples/python/github/main.py"
```

脚本需要执行位；复制后执行 `chmod +x main.py`。用 `wayrun plugin check <script.lua>` 检查 Lua 示例。Python 的框架与 API 见工作区的 [Python 指南](https://github.com/Prslc/WayRun-Plugins/blob/main/docs/zh_cn/PYTHON_CN.md)。

这些是示例，不是审核过的插件包；注册前先读一遍。
