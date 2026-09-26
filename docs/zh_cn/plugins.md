# 插件

`~/.config/wayrun/plugins.toml` 是插件注册表。首次运行时自动生成一份带注释的模板，并且会被
监听，修改后无需重启即可生效。

## 条目字段

```toml
[[plugins]]
id = "runner"
keyword = "r"
enabled = true
```

| 字段 | 必填 | 含义 |
| --- | --- | --- |
| `id` | 是 | 该条目配置的插件：内置 id，或外部主机上报的 id。 |
| `keyword` | 是 | 路由到该插件的前缀。`""` 表示它是**默认**提供者。 |
| `enabled` | 否 | 默认 `true`。设为 `false` 可禁用而不删除条目。 |
| `command` | 否 | 外部 JSON-RPC 2.0 主机，见下文。 |
| `resident` | 否 | 默认 `false`。跨调用保留主机进程，见下文。 |

调整条目顺序即可改变优先级。既不是内置 id、也不是外部主机的 id 会被忽略。

## 路由

只有某个插件拥有输入的第一个词作为 keyword 时，才会路由到该插件；否则整段输入（含第一个词）都作为
默认提供者的查询：`foo bar` 会以 `foo bar` 到达默认提供者，而不是 `bar`。

默认提供者（keyword 为 `""`）全部作答并合并：先按每行的相关度（命中档位的权重 × 该行命中的面），
其次按该行的使用次数，最后按条目顺序。因此强度相当的名字命中会排到描述命中之前，而一个精确的关键字
命中仍会排在只是恰好包含查询词的名字命中之前。

## 内置插件

| Id | Keyword | 搜索内容 | 依赖 |
| --- | --- | --- | --- |
| `calculator` | `""` | 行内算术。 | — |
| `system-commands` | `""` | `lock`、`reboot`、`shutdown`、`suspend`、`logout`。 | — |
| `app-search` | `""` | 已安装应用（desktop 条目）：名字、名字的近似拼写，以及各元数据面按各自能承载的强度参与匹配（关键字与通用名按词、描述按前缀）。 | — |
| `runner` | `r` | 模糊匹配 `$PATH` 可执行文件；可带参数。命中已安装应用时经 GLib 启动（遵循 `Terminal=`），其余在终端中运行。 | 终端模拟器 |
| `file-search` | `f` | 主目录下的文件，任意深度。 | — |
| `path-search` | `d` | 主目录下的目录，任意深度。 | — |
| `clipboard` | `c` | 剪贴板历史。 | `cliphist` 正在运行 |
| `window` | `w` | 打开窗口（niri 或 Hyprland，取决于编译的后端）。 | 对应的合成器后端 |

依赖缺失时，对应关键词返回空结果，而不是报错。

`file-search` 与 `path-search` 经索引覆盖整个主目录。`config.md` 的 `[files]` 中设
`index = false` 则只搜索主目录的 `depth` 层，且不留缓存。

## 示例插件

[WayRun-Plugins](https://github.com/Prslc/WayRun-Plugins) 工作区在 Python 示例旁
提供了 `firefox.lua`（书签与历史，读取最新 profile 的 `places.sqlite` 单一份快照）
与 `web.lua`（`config.toml` 中所设引擎的联想词）。它们是普通的外部主机——由启动器
二进制运行的 Lua 脚本——与其他主机一样注册：

```toml
[[plugins]]
id = "firefox-bookmarks"
keyword = "b"
command = "/path/to/WayRun-Plugins/firefox.lua"
resident = true
```

`id` 取脚本声明的插件之一；`h` 用同一个 `command` 注册 `firefox-history`。脚本能做
什么见工作区的
[Lua 插件](https://github.com/Prslc/WayRun-Plugins/blob/main/docs/zh_cn/LUA_CN.md)。

## 结果动作

结果行可以携带次级命令，显示在 `Shift+Enter` 二级菜单中。不同类型的菜单各不相同：
`file-search`/`path-search` 提供“在终端中打开”“在文件管理器中显示”与“复制路径”，
`app-search` 列出该条目的 `[Desktop Action …]`，外部主机的行则按所带的 `actions`
提供（工作区的 `web.lua` 提供“复制链接”），
每个可操作的行都会补上启动器级别的置顶/取消置顶，空查询历史来源的行还会多一条“从历史中移除”。
菜单最前是行自身的命令，外部主机自带的动作排在插件自身动作之后，启动器级条目在最后。

给动作填上 `id`，用户就能用 `Alt+Enter` 把它设为默认 Enter 动作；菜单里的 **打开**（行原本的
命令）在这个手势下表示取消默认。见 [jsonrpc.md](jsonrpc.md#结果项) 的 `actions` 字段。

## 外部主机

`command` 指向一个 JSON-RPC 2.0 主机：单个可执行文件 token，按 `PATH` 解析或写绝对路径，
不含参数、无 shell 语法（脚本需 shebang + 执行位）。每次调用都会重新启动主机，并受一个较短的
超时约束，因此主机卡死不会拖住启动器。

`resident = true` 则改为跨调用保留同一个主机进程：每次调用约 1ms，而新起进程要几毫秒。启动器
关闭时、空闲两分钟后、或调用崩溃/卡死时，进程会被回收，下一次调用重新启动。两种模式下主机都必须
对单次调用无状态，但常驻进程内可以自行缓存。

身份 `icon` 与每条结果的 `icon` 都必须是主机**自己准备**的图标文件的绝对路径，动作的 `icon`
与行的 `badge` 同理。主题图标名、`papirus:` 规范、`builtin:` 字形一律视为无图标。结果行自身
没有图标时，改用所属插件的身份图标；身份图标也没有时，回退到编入二进制的占位图。

主机可以手写。[WayRun-Plugins](https://github.com/Prslc/WayRun-Plugins) 工作区提供了一套
Python 框架、Lua 与 Python 示例及模板（`template/`、`template.lua`），可复制起步。
主机协议是 [jsonrpc.md](jsonrpc.md)
中记录的 JSON-RPC 子集：`search`、`top`、`select`、`forget`、`list_plugins`。
