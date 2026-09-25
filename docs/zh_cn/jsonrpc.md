# JSON-RPC 2.0

后端（`wayrun --core`，或 `wayrun-core` 符号链接）通过 stdin/stdout 只讲
[JSON-RPC 2.0](https://www.jsonrpc.org/specification)：每一行都必须是一条 JSON-RPC 消息，
不是合法 JSON 的行会收到标准的 `-32700` 解析错误。响应与通知都是 stdout 上以换行分隔的 JSON。

```sh
printf '%s\n' '{"jsonrpc":"2.0","method":"search","params":{"text":"firefox"},"id":1}' | wayrun --core
# -> {"jsonrpc":"2.0","result":[...],"id":1}
```

| 方法 | 参数 | 结果 |
|------|------|------|
| `search` | `{"text"}` | 结果项数组；不带 `id` 时改为推送 `results` 通知 |
| `top` | — | 最常用项；不带 `id` 时同样推送 `results` 通知 |
| `dismiss` | — | `null`（启动器已关闭：丢弃记住的载荷与仍在飞行中的搜索） |
| `select` | 结果项对象 | `null`（记录使用；`ephemeral` 与 `copy` 行不记录） |
| `command` | 一个 [`Action`](#动作) 对象 | `null`（执行一条行或面板命令） |
| `pin` | `{"scope","on_click": Action}` | `{"pinned": bool}`（把该查询载荷中的一行置顶） |
| `unpin` | `{"scope","on_click": Action}` | `{"unpinned": bool}` |
| `default` | `{"scope","action_id"}` | `null`（记录某插件的默认 Enter 动作；`action_id` 为 null 则清除） |
| `forget` | `{"on_click": Action}` | `{"forgotten": bool}` |
| `list_plugins` | — | 插件元数据；见 [schema](#插件元数据list_plugins) |
| `theme` | — | 主题颜色 |
| `ping` | — | `"pong"` |

不带 `id` 的请求是通知：只产生副作用，不返回响应。未知方法返回 `-32601`，请求格式
错误返回 `-32600`，参数非法返回 `-32602`，非 JSON 行返回 `-32700`。

## 通知

除响应外，后端还会推送不带 `id` 的 JSON-RPC 通知：

| 方法 | 参数 | 含义 |
|------|------|------|
| `theme` | 主题颜色 | 解析后的主题；连接时发一次，调色板变化时再发 |
| `results` | 结果项数组 | 一次搜索的结果 |

`search` 和 `top` 不带 `id` 时走流式：后端先中止上一个搜索，再推送 `results` 通知；
带 `id` 时则同步返回数组，方便一次性客户端直接取回结果。

## 动作

`Action` 是用 `type` 字段标记的单个对象（`{"type": …}`），描述一行要执行什么。它既作
`command` 方法的参数，也是结果项 `on_click` 的类型、面板 `execute` 动作所携带的内容：

| `type` | 字段 | 效果 |
|--------|------|------|
| `run` | `cmd` | 执行 shell 命令（整条 shell 行） |
| `run_in_terminal` | `cmd` | 在终端模拟器中执行 shell 命令 |
| `launch` | `desktop_id` | 经 GLib 的 `GAppInfo` 按 desktop id 启动应用 |
| `copy` | `text` | 写入 Wayland 剪贴板 |
| `desktop_action` | `desktop_id`、`action_id` | 运行一个 `[Desktop Action …]` 组 |
| `open` | `uri` | 交给默认处理器打开（URL、`file:` 或 `mailto:`） |
| `reveal` | `uri` | 在文件管理器中定位文件（仅面板可见） |
| `terminal` | `uri` | 在 URI 所在目录打开终端，文件则用其父目录（仅面板可见） |

`run` 的 `cmd` 是整条 shell 行；`launch` 与 `desktop_action` 的 id 不加引号。
`file:` URI 会做百分号编码，路径里的空格和非 ASCII 字符都能保留；`Terminal=true`
的处理器会在终端中启动。

`search` 的 `params` 是带 `text` 键的对象（非空字符串）。缺省、空 `text`、裸字符串、
`{"query": …}`、非字符串 `text` 都会返回 `-32602`。最常用项请用 `top`——`search`
不承担默认视图。

`forget` 把一行从使用历史中移除。如果 `on_click` 是 `run` 动作，且命令的第一个词恰好
是某个已注册外部主机的 `command`，后端还会向该主机转发一条 `forget`，让它清理自己的数据——
比如 todo 插件据此删掉对应待办。返回值表示是否真的删掉了东西：删掉一条历史行、**或**拥有该行的
主机正常应答，都是 `true`，否则为 `false`。未实现 `forget` 的主机会回 `-32601`，这算
“不是我的行”——启动器会把这类行留在列表里，不会谎称删除成功。

`pin` 以**精确查询字符串**为范围（`scope` 是整段去掉首尾空白的输入；`""` 表示空查询历史），
以 `on_click` 指名的命令为键保存：后端在它最近为该 scope 发出的载荷里查找这一行，客户端不必
把结果项回传；载荷中已没有该命令时返回 `pinned: false`。`unpin` 用同样的键删除。之后某次
`search` 的 `text` 去掉首尾空白后与该字符串相等时，才会把这些置顶项按最近置顶优先排在最前，
与新鲜结果去重，并补上各自的 `actions`；只输入关键词不会命中。

## 插件元数据（`list_plugins`）

`list_plugins` 返回插件对象数组，存在两种形态：

### 后端 → 客户端

在后端的 stdin 上调用，返回当前注册表：

| 键 | 类型 | 含义 |
|-----|------|------|
| `id` | string | 插件 id，与 `plugins.toml` 条目对应 |
| `name` | string | 显示名 |
| `icon` | string | 图标绝对路径 |
| `keyword` | string | 触发前缀（空 = 默认） |
| `enabled` | bool | 插件是否启用 |

### 外部主机 → 后端（身份发现）

当 `plugins.toml` 条目声明 `command` 时，后端启动主机并调用一次 `list_plugins`
以发现身份。遵循此契约的 Python 框架与示例主机见
[WayRun-Plugins](https://github.com/Prslc/WayRun-Plugins) 工作区；响应 `result`
为对象数组：

| 键 | 类型 | 含义 |
|-----|------|------|
| `id` | string（必填） | 插件 id——必须与 `plugins.toml` 条目的 id 一致，否则身份被忽略 |
| `name` | string | 显示名（空 → 回落为配置的 id） |
| `icon` | string | 主机自带图标的绝对路径（见 [图标规范](#图标规范)） |
| `description` | string | ready 提示，显示在 `?` 列表与关键词+空格提示中 |

未实现 `list_plugins`（或返回中没有匹配的 `id`）的主机仍可用——搜索照常转发、
结果照常解析——但身份退化为配置的 id 且无图标，因此 `?` 列表与关键词+空格提示
显示默认占位符。

## 关键词插件的默认视图

`plugins.toml` 中带非空 `keyword` 的条目，由「关键词 + 空格」（如 `todo `）的查询
打开——`search` 方法与流式通知皆然。打开时后端会向外部主机请求**默认视图**：

```sh
printf '%s\n' '{"jsonrpc":"2.0","method":"top","params":{"plugin":"todo"},"id":1}' | /path/to/todo/main.py
# -> {"jsonrpc":"2.0","result":[{"title":…,"summary":…,"on_click":…,"icon":…}],"id":1}
```

注意这是后端 → 主机的请求，与后端自身的 `top` RPC（返回最常用的使用历史，服务启动器
的空查询视图）不同。主机通过响应 `top` 声明默认视图（用插件框架的
`@plugin.method("top")` 注册）；响应 `result` 为结果项数组，与 `search`
同一套 [schema](#结果项)，图标同样会被解析。

主机返回非空默认视图时，它取代关键词+空格的身份提示；否则——主机未实现 `top`
（未知方法 `-32601`）、处理失败（`-32603`），或返回空结果列表——提示来自
`list_plugins`（`name` + `description`），这也是插件的空状态。

后端自己的文案（动作名、启动器的说明行）会按界面语言翻译；主机发来的字符串原样透传，
所以主机的身份与结果行用什么语言，由主机自己决定。

## 结果项

`search` 和 `top` 返回结果项数组。每个结果项是含以下键的对象——前五个**始终都在**，
缺省的可选字段为 `null`（而非省略）；`actions`/`badge` 仅在设置时出现：

| 键 | 类型 | 含义 |
|-----|------|------|
| `title` | string | 主标签（应用名、命令、文件名……） |
| `summary` | string \| null | 副行（命令、路径、描述……） |
| `on_click` | [`Action`](#动作) \| null | Enter 绑定的动作 |
| `icon` | string \| null | 图标图像的绝对路径；见 [图标规范](#图标规范) |
| `ephemeral` | bool | 为 true 时，选中该项不记入使用历史 |
| `actions` | array | 可选，`Shift+Enter` 二级菜单的次级命令 |
| `badge` | string \| null | 可选，行右缘的状态图标（置顶行为图钉） |

`actions` 元素为 `{"title": string, "action": PanelAction, "icon"?: string}`，
`icon` 与结果行的 `icon` 采用同样的规范解析。动作还可能带 `id`（稳定 kind）、
`plugin`（归属插件，用于限定默认动作的作用域）与 `default`（为 true 表示 Enter 执行它）：
`plugin` 与 `default` 由后端赋值，主机传来的这两个字段会被忽略；外部主机给自己的动作填 `id`
即可使其可被设为默认。
`PanelAction` 是以下之一：

| `type` | 字段 | 含义 |
|--------|------|------|
| `execute` | `command` | 执行该 [`Action`](#动作) |
| `pin` | `scope` | 把该行的命令置顶到某条精确查询 |
| `unpin` | `scope`、`on_click` | 从某条精确查询取消置顶该命令 |
| `forget` | `on_click` | 把该命令从使用历史中移除 |

后端会为每个可操作的行补上启动器级别的置顶/取消置顶；对源自空查询历史、且本可记入
历史（非 `ephemeral`、非 `copy`）的行，再补一条移除历史。产出该行的内置提供者会补上
类型专属动作（文件定位、复制路径、在终端打开、`[Desktop Action …]`、复制链接），
外部主机自带的动作排在其后。

无 `on_click` 的结果项不可交互（仅展示）。

选中一项会把它记入使用历史（空查询时的 `top` 列表）。有两类行不记：主机标记
`ephemeral: true` 的行（如一次性的搜索结果），以及 `on_click` 为 `copy` 动作的行
（它代表被复制的文本，而不是可再次打开的目标）。

### 图标规范

每个 `icon` 都是绝对路径。**外部插件主机**（`plugins.toml` 中带 `command` 的条目）必须
返回它**自己准备**的图标文件的绝对路径：结果项 `icon` 字段（`search` 结果与 `top`
默认视图皆然）、动作的 `icon`、行的 `badge`，以及 `list_plugins` 身份 `icon` 都如此。
主题图标名、`papirus:` 规范、`builtin:` 字形一律视为无图标。

结果行图标缺失或为符号规范时，回退到插件的身份图标；身份图标也没有时，绘制内置占位图标。
