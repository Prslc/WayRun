# JSON-RPC 2.0

## 协议基础

后端（`wayrun --core`，或 `wayrun-core` 符号链接）只通过 stdin/stdout 讲 [JSON-RPC 2.0](https://www.jsonrpc.org/specification)，不接受别的形式。每一行必须是一条 JSON-RPC 消息。应答与通知以每行一条 JSON 的形式写到 stdout。

```sh
printf '%s\n' '{"jsonrpc":"2.0","method":"search","params":{"text":"firefox"},"id":1}' | wayrun --core
# -> {"jsonrpc":"2.0","result":[...],"id":1}
```

不是有效 JSON 的行会得到标准的 `-32700` 解析错误。

## 后端方法

| 方法 | 参数 | 结果 |
|--------|--------|--------|
| `search` | `{"text"}` | 结果项数组；不带 `id` 发送时改为流式，用 `results` 通知返回 |
| `dismiss` | — | `null`（启动器已关闭：丢弃仍在进行的搜索） |
| `record` | `{"on_click": Action}` | `null`（增加该命令的使用次数；`copy` 命令不记录） |
| `command` | 一个 [`Action`](#动作) 对象 | `null`（执行一条结果行或面板命令） |
| `default` | `{"scope","action_id"}` | `null`（记住插件的默认 Enter 动作；`action_id` 为 null 时清除） |
| `list_plugins` | — | 插件元数据；见[结构](#插件元数据-list-plugins) |
| `theme` | — | 主题颜色 |
| `ping` | — | `"pong"` |

不带 `id` 的请求是通知：只产生副作用，不应答。未知方法返回 `-32601`，请求格式错误返回 `-32600`，参数错误返回 `-32602`，非 JSON 行返回 `-32700`。

## 通知

后端也会推送不带 `id` 的 JSON-RPC 通知：

| 方法 | 参数 | 含义 |
|--------|--------|---------|
| `theme` | 主题颜色 | 解析后的主题，在连接时以及每次调色板变化时发送 |
| `results` | 结果项数组 | 一次搜索结果 |
| `results_partial` | 结果项数组 | 流式搜索的中间结果；搜索仍在进行 |
| `image_ready` | `{"path": "…"}` | 后台图片获取完成；界面重新加载该路径 |

不带 `id` 发送的 `search` 是流式：后端中止任何挂起的搜索，然后用 `results` 通知应答。流式宿主可以在最终结果前发送 `results_partial` 通知。带 `id` 发送时，`search` 同步返回数组，供一次性客户端使用。

## 动作

`Action` 是按 `type` 字段标记的一个对象（`{"type": …}`），描述结果行会执行什么。它出现在三处：`command` 的参数、结果的 `on_click`，以及面板条目的 `action`。

### 动作类型

| `type` | 字段 | 效果 |
|--------|--------|--------|
| `run` | `cmd` | 执行一条 shell 命令（一整行） |
| `run_in_terminal` | `cmd` | 在终端模拟器中执行一条 shell 命令 |
| `launch` | `desktop_id` | 通过 GLib 的 `GAppInfo` 按 desktop id 启动应用 |
| `copy` | `text` | 把文本写入 Wayland 剪贴板 |
| `desktop_action` | `desktop_id`, `action_id` | 执行一个 `[Desktop Action …]` 分组 |
| `open` | `uri` | 用默认处理程序打开 URI（URL、`file:` 或 `mailto:`） |
| `open_with` | `desktop_id`, `uri` | 用指定的处理程序而不是默认程序打开 URI |
| `reveal` | `uri` | 在文件管理器中显示文件（仅面板） |
| `terminal` | `uri` | 在 URI 所在目录打开终端；文件则用其父目录（仅面板） |
| `plugin` | `plugin`, `action`, `payload` | 把命名的命令与 payload 交给该插件的宿主 |

### run 与字段码

`plugin` 动作的 payload 是任意 JSON 值，由指定的插件自己解释。后端只负责路由；宿主在插件自己的作用域中运行它（见[命令](#命令)）。

`run` 的 `cmd` 是一整条 shell 命令行，启动器不会改写它。因此 `%u` 之类的字段码会原样到达 shell。只有 `launch` 与 `desktop_action` 会读取 `.desktop` 字段码，两者接收的 id 都不带引号。

文件 URI 会被百分号编码，因此含空格或非 ASCII 字符的路径能够保留。处理程序带 `Terminal=true` 时会在终端中启动。

### search 参数

`search` 接受一个对象，`text` 键必须是非空字符串。缺少 `params`、`text` 为空、直接传字符串、传 `{"query": …}`，或 `text` 不是字符串，都会返回 `-32602`。

`text` 为空的通知会取消任何挂起的搜索，因此清空的输入框不会收到过期结果。

## 插件元数据（`list_plugins`）

`list_plugins` 返回一组插件对象，有两种结构。

### 后端 → 客户端

在后端的 stdin 上调用时，返回当前注册表：

| 键 | 类型 | 含义 |
|-----|------|---------|
| `id` | string | 插件 id，与 `plugins.toml` 的条目一致 |
| `name` | string | 显示名称 |
| `icon` | string | 标识图标的规范：绝对路径或 `builtin:` 字形 |
| `keyword` | string | 触发前缀（空 = 默认） |
| `enabled` | bool | 插件是否启用 |

### 外部宿主到后端（身份发现）

`plugins.toml` 条目声明 `command` 时，后端会启动宿主并调用一次 `list_plugins` 来发现身份。讲这套契约的 Python 框架与示例宿主在 [WayRun-Plugins](https://github.com/Prslc/WayRun-Plugins) 工作区；Lua 插件由启动器自己承载，也应答同一个调用，见 [lua.md](lua.md)。应答的 `result` 是一组对象：

| 键 | 类型 | 含义 |
|-----|------|---------|
| `id` | string（必填） | 插件 id，必须与 `plugins.toml` 条目一致，否则这条身份信息会被忽略 |
| `name` | string | 显示名称（为空时回退到配置的 id） |
| `icon` | string | 宿主自带图标的绝对路径（见[图标规范](#图标规范)） |
| `description` | string | 就绪提示，显示在 `?` 列表与「关键词+空格」提示中 |
| `api` | number | 宿主面向的插件 API；缺失表示 `0`，即版本机制出现之前的写法 |

`api` 会与启动器自己的插件 API 版本比较。不一致时只在 stderr 记一条日志，不会拒绝插件，插件可能仍然可用；版本号何时变化的规则见 [plugins.md](plugins.md)。

没有 `list_plugins`，或 id 与条目不匹配的宿主仍然可用：搜索会被转发，结果也会解析。它的身份信息回退为配置的 id，且没有图标，因此 `?` 与「关键词+空格」提示显示默认占位。

## 关键词插件的默认视图

`plugins.toml` 中关键词非空的条目，在查询恰好是「关键词加一个空格」（例如 `todo `）时打开。`search` 方法与流式通知都走这条路径。打开关键词时，后端向外部宿主请求它的**默认视图**：

```sh
printf '%s\n' '{"jsonrpc":"2.0","method":"top","params":{"plugin":"todo"},"id":1}' | /path/to/todo/main.py
# -> {"jsonrpc":"2.0","result":[{"title":…,"summary":…,"on_click":…,"icon":…}],"id":1}
```

这个 `top` 调用由后端发给宿主。宿主通过支持 `top` 来声明默认视图，在插件框架里用 `@plugin.method("top")` 注册。应答的 `result` 是一组结果项，结构与 `search` 相同（[结果项](#结果项)），也包含图标。

宿主返回非空默认视图时，它会替换「关键词+空格」的身份提示。其他情况下提示来自 `list_plugins`（`name` + `description`），这也是插件的空状态。这些情况包括：宿主没有 `top`（未知方法 `-32601`）、处理程序失败（`-32603`），或结果为空。

后端翻译自己的字符串：动作标题与启动器的帮助行。宿主发来的内容原样转发，因此身份信息与结果行的语言由宿主自己负责。

## 命令

结果行的 `on_click`（或动作）为 `{"type":"plugin","plugin":"todo", …}` 时，后端不执行任何东西，而是把命名的命令交回它的宿主：

```sh
printf '%s\n' '{"jsonrpc":"2.0","method":"command","params":{"plugin":"todo","action":"done","payload":{"id":7}},"id":1}' | /path/to/todo/main.py
# -> {"jsonrpc":"2.0","result":null,"id":1}
```

动作名与 payload 由插件自己定义。后端只负责路由，并在插件作用域中运行处理程序，因此 Lua 脚本的 `fs.write`、`env` 与 `exec` 与搜索时一致。命令运行时启动器已经在关闭，因此失败写入日志，不显示在列表里；应答本身被忽略。

## 流式搜索

支持流式的后端会在 `search` 请求的 params 里发送 `"stream": true`。宿主随后可以在最终应答前发送任意多条通知：

```json
{"jsonrpc":"2.0","method":"partial","params":{"id":17,"rows":[ … ]}}
```

`rows` 是与应答结构相同的结果项数组，`id` 是请求自己的 id。每条通知替换屏幕上的结果行。最终应答为准，并结束搜索。不支持流式的宿主不发送这些通知；旧版后端从不发送 `stream`，因此宿主把该标志缺失视为「只应答一次」。

## 结果项

`search` 与宿主的 `top` 都返回一组结果项。每一项都是对象，字段如下。`title`、 `summary`、`on_click` 与 `icon` 始终存在，缺失的可选字段为 `null`；`preview`、 `actions` 与 `icon_shape` 只在设置时出现。

### 字段

| 键 | 类型 | 含义 |
|-----|------|---------|
| `title` | string | 主标签（应用名、命令、文件名等） |
| `summary` | string \| null | 次要行（命令、路径、描述等） |
| `on_click` | [`Action`](#动作) \| null | 绑定到 Enter 的动作 |
| `icon` | string \| null | 图标规范：绝对路径或 `builtin:` 字形；宿主结果可以是 `https://` URL，由后端缓存；见[图标规范](#图标规范) |
| `icon_shape` | string | 可选；`"circle"` 把图标框裁成圆形，缺省时按原样绘制 |
| `preview` | object | 可选的分栏预览，`{"type":"image","path":"/abs/file.png"}` 或 `{"type":"text","text":"…","format":"markdown"}`；宿主的图片路径可以是 `https://` URL |
| `actions` | array | 可选的次要命令，用于 `Shift+Enter` 动作面板 |

### 预览

`preview` 与 `Action` 一样按 `type` 标记。`image` 携带绝对路径，或宿主给出的 `https://` URL（由后端获取并存入图片缓存）。它支持 PNG、SVG、JPEG、WebP、GIF 与 BMP，显示在选中行右侧的栏中。`text` 携带 UTF-8 文本，由启动器排版，上限 64 KiB。

文本预览的可选 `format` 为 `"plain"`（缺省值）或 `"markdown"`。未知值会让结果行失败，与未知 `icon_shape` 相同。Markdown 会渲染标题、粗体、斜体、删除线、行内代码、代码块、有序与无序列表、引用、分隔线与链接文字。表格、图片、行内 HTML 与任务列表会在同一栏中退化为纯文本。旧版启动器忽略 `format`，按纯文本显示原始内容。主题名或 `builtin:` 字形不能作为预览。

### 动作条目

`actions` 的条目要么是命令：`{"title": string, "action": Action, "icon"?: string}`；要么是分组：`{"title": string, "children": [ …条目… ]}`，按 Enter 会展开动作面板的下一层。图标解析规则与结果行的 `icon` 相同。

条目还可以携带 `id`（稳定的类型）、`plugin`（所属插件，用于限定记住的默认动作）与 `default`（Enter 会执行时为 true）。`plugin` 与 `default` 由后端赋值，宿主提供的值会被忽略。宿主给动作设置 `id` 才能让用户把它设为默认，分组条目也一样。

所属内置提供者会补上类型专属条目：文件定位、复制路径、在终端中打开、系统提供的其他打开方式、`[Desktop Action …]` 分组、复制链接。宿主自带的条目排在它们之后。

没有 `on_click` 的结果项不可交互，只用于显示。

### 使用次数

`record` 会增加 `on_click` 所命名命令的启动次数。默认提供者的结果打平时，按启动次数排序。`copy` 命令不计入：它的值是复制的文本，不是可再次打开的目标。

### 图标规范

界面层收到的每个 `icon` 都是绝对路径，或启动器从内置集合绘制的 `builtin:<name>` 字形。

**外部宿主**（`plugins.toml` 中声明 `command` 的条目）可以把结果自己的 `icon` 返回为 `https://` URL。后端会在后台获取该图片并存入 `$XDG_CACHE_HOME/wayrun/icons`，再把缓存路径交给界面层。预览的图片 `path` 也可以是同样处理的 `https://` URL。

标识 `icon`、动作的 `icon` 与预览的本地路径仍必须是宿主自带的文件。`builtin:` 字形、主题图标名或 `papirus:` 规范都视为没有图标。

结果行图标缺失或是符号图标时，回退到插件的标识图标。标识图标也缺失时，启动器绘制内置占位图。
