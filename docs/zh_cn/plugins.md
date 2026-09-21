# 插件

`~/.config/wayrun/plugins.toml` 是插件注册表。首次运行时自动生成（即 `core/default-plugins.toml` 的副本），
并且会被监听，修改后无需重启后端即可生效。

## 条目字段

```toml
[[plugins]]
id = "runner"
keyword = "r"
enabled = true
```

| 字段 | 必填 | 含义 |
| --- | --- | --- |
| `id` | 是 | 该条目配置的插件。可以是内置 id，也可以是外部主机上报的 id。 |
| `keyword` | 是 | 路由到该插件的前缀。`""` 表示它是**默认**提供者。 |
| `enabled` | 否 | 默认 `true`。设为 `false` 可禁用而不删除条目。 |
| `command` | 否 | 外部 JSON-RPC 2.0 主机，见下文。 |

调整条目顺序即可改变优先级。未识别或已删除的 id 会被忽略。内置 id 且未写 `command` 时使用编译进的插件；
未知 id 且未写 `command` 时跳过。

## 路由

只有某个插件拥有输入的第一个词作为 keyword 时，才会路由到该插件；否则整段输入（含第一个词）都作为
默认提供者的查询：`foo bar` 会以 `foo bar` 到达默认提供者，而不是 `bar`。

默认提供者（keyword 为 `""`）按条目顺序尝试，第一个返回非空结果者胜出，因此裸查询是应用/命令搜索，而非所有插件的并集。

## 内置插件

| Id | Keyword | 搜索内容 | 依赖 |
| --- | --- | --- | --- |
| `calculator` | `""` | 行内算术。 | — |
| `system-commands` | `""` | `lock`、`reboot`、`shutdown`、`suspend`、`logout`。 | — |
| `app-search` | `""` | 已安装应用（desktop 条目）。 | — |
| `runner` | `r` | 模糊匹配 `$PATH` 可执行文件；可带参数。命中已安装应用时经 GLib 启动（遵循 `Terminal=`），其余在终端中运行。 | 终端模拟器 |
| `firefox-bookmarks` | `b` | Firefox 书签。 | 已配置 profile 的 Firefox |
| `firefox-history` | `h` | Firefox 历史。 | 已配置 profile 的 Firefox |
| `web-search` | `s` | 网页搜索建议（引擎在 `config.toml` 中设置）。 | 可访问网络 |
| `file-search` | `f` | 主目录下的文件。 | — |
| `path-search` | `d` | 主目录下的目录。 | — |
| `clipboard` | `c` | 剪贴板历史。 | `cliphist` 正在运行 |
| `window` | `w` | 打开窗口（niri 或 Hyprland，取决于编译的后端）。 | 对应的合成器后端 |

依赖缺失时，对应关键词返回空结果，而不是报错。

## 结果动作

结果行可以携带次级命令，显示在前端的 `Shift+Enter` 二级菜单中。菜单由拥有该行的插件定义，
而非前端，因此不同类型的菜单各不相同：`file-search`/`path-search` 提供“在文件管理器中显示”、“复制路径”
与“在终端中打开”，`app-search` 列出该条目的 `[Desktop Action …]`，`web-search` 与 Firefox 插件提供“复制链接”，
没有自带动作的插件至少也有启动器级别的置顶/取消置顶；空查询历史会为它来源的行补上
“从历史中移除”。外部主机可以在
结果项上输出自己的 `actions` 数组，后端会把它们排在内置项之后。给动作填上 `id`，
用户就能用 `Alt+Enter` 把它设为默认 Enter 动作。见
[jsonrpc.md](jsonrpc.md#结果项) 的 `actions` 字段。

## 外部主机

`command` 指向一个 JSON-RPC 2.0 主机。该值必须是单个可执行文件 token，按 `PATH` 解析或写绝对路径，
不含参数、无 shell 语法（脚本需 shebang + 执行位）。

后端每次查询都会重新启动主机（一次请求后关闭其 stdin），并受 5s 超时约束，因此主机卡死只会消耗本次截止时间，
不会拖垮会话。主机经 `list_plugins` 上报的身份会按文件的 `(mtime, size)` 缓存，因此后续启动不会为未变的主机再次 fork。

身份 `icon` 与每条结果的 `icon` 都接受绝对路径、`papirus:` 规范或主题图标名；后端会在结果行到达前端之前把所有规范解析为绝对路径。结果行自身没有可用图标或规范解析不到时，改用所属插件的身份图标；身份图标也解析不到时，才回退到编入二进制的占位图。

主机可以手写。[WayRun-Plugins](https://github.com/Prslc/WayRun-Plugins) 工作区提供了一套 Python 框架、
示例插件与 `template/` 模板，可复制起步。主机协议是 [jsonrpc.md](jsonrpc.md) 中记录的 JSON-RPC 子集：
`search`、`top`、`select`、`forget`、`list_plugins`。
