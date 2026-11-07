# 插件

`$XDG_CONFIG_HOME/wayrun/plugins.toml`（默认 `~/.config/wayrun/plugins.toml`）是插件注册表。首次运行按内置默认值生成，启动器会监听该文件，修改后无需重启。

## 注册表命令

`wayrun plugin list` 打印生效的注册表：id、名称、关键词、图标以及被禁用的条目。你的文件已经合并到内置条目之上。

- `wayrun plugin add <script.lua> [--keyword K] [--resident]`：注册一个 Lua 插件。它会先加载脚本，拒绝缺少 shebang 或执行位、id 已配置、关键词已被其他启用条目占用的情况，然后追加条目；关键词默认取插件 id。
- `wayrun plugin enable|disable <id>`：写入或修改某个 id 的用户条目，内置插件同样适用。
- `wayrun plugin set-keyword <id> <keyword>`：用同样方式修改路由关键词。关键词为空时，插件进入默认链。
- `wayrun plugin remove <id>`：删除用户条目；内置插件回到内置默认。

所有修改都会保留文件里的注释，并立即写入。运行中的启动器通过监听器读取新文件。

## 条目字段

```toml
[[plugins]]
id = "runner"
keyword = "r"
enabled = true
```

| 字段 | 必填 | 含义 |
| --- | --- | --- |
| `id` | 是 | 指定哪个插件：内置插件 id，或外部宿主上报的 id。 |
| `keyword` | 是 | 把输入路由到该插件的前缀。`""` 表示它是**默认**提供者。 |
| `enabled` | 否 | 默认 `true`。`false` 禁用插件，但保留条目。 |
| `command` | 否 | 一个外部 JSON-RPC 2.0 宿主，见下文。 |
| `resident` | 否 | 默认 `false`。跨调用保留宿主进程，见下文。 |

调整条目顺序即可改变优先级。既不是内置插件、也不对应外部宿主的 id 会被忽略。

## 路由

只有当某个插件拥有首个词作为关键词时，这个首词才把查询路由给该插件。否则整段输入（包括首个词）作为默认提供者的查询：`foo bar` 以 `foo bar` 到达默认提供者，而不是 `bar`。

所有默认提供者（关键词 `""`）都会应答，启动器分三步合并它们的结果行。先按相关度排序：匹配类型的权重乘以命中的元数据项。相关度相同时比较使用次数，最后按条目顺序。因此标题命中会排在强度相当的描述命中之前。精确关键词仍然胜过只是包含查询的标题。

无法给条目打分的提供者（例如外部宿主）改为直接列出结果。它们排在所有已打分的结果行之后，按提供者自己的顺序填充剩余位置。在默认链中，无关键词的外部宿主有四分之一秒的超时，无响应的宿主因此不会拖慢卡片。

## 内置插件

| id | 关键词 | 搜索内容 | 依赖 |
| --- | --- | --- | --- |
| `calculator` | `""` | 即时计算。 | — |
| `system-commands` | `""` | `lock`、`reboot`、`shutdown`、`suspend`、`logout`。 | — |
| `app-search` | `""` | 已安装应用（desktop 条目）：名称、名称的近似写法，以及各项元数据按各自能承受的强度匹配（关键词或通用名按词匹配，描述按前缀）。 | — |
| `path-lookup` | `""` | 路径查询（`~…`、`/…` 或含 `/`）：存在的路径本身，或所输入目录中匹配的条目；普通查询没有提供者命中时，也列出 `$HOME` 第一层。 | — |
| `runner` | `r` | 模糊搜索 `$PATH` 可执行文件，接受参数。已安装应用通过 GLib 启动（遵循 `Terminal=`），其余在终端中运行。 | 终端模拟器 |
| `file-search` | `f` | 主目录下任意深度的文件。 | — |
| `path-search` | `d` | 主目录下任意深度的目录。 | — |

依赖缺失的关键词返回空结果，不会失败。

`file-search` 与 `path-search` 通过索引覆盖整个主目录。`config.md` 的 `[files] index = false` 把它们限制在 `$HOME` 下几层，此时不保留缓存。

## 结果动作

结果行可以携带次要命令，显示在 `Shift+Enter` 面板中。不同结果类型提供的内容不同：

- `file-search` 与 `path-search` 提供「在终端中打开」「在文件管理器中显示」「复制路径」，然后是一个**用其他应用打开**分组，包含系统提供的其他打开方式，并会展开下一层面板。
- `app-search` 列出条目的 `[Desktop Action …]` 分组。
- 外部宿主的结果行携带它附加的 `actions`；例如工作区的 `web.lua` 提供「复制 URL」。

行自身的命令排在面板最前。外部宿主自带的动作排在插件动作之后。

给动作设置 `id`，用户就能用 `Alt+Enter` 把它设为默认的 `Enter` 动作。面板里的**打开**条目（行自身的命令）接受同一手势时，表示清除默认。`actions` 字段见 [jsonrpc.md](jsonrpc.md#结果项)。

## 外部宿主

外部宿主是 `command` 指定的独立可执行文件，通过 stdin/stdout 应答 JSON-RPC。

### 命令与超时

`command` 指定一个 JSON-RPC 2.0 宿主：只写可执行文件本身（不含参数），按 `PATH` 解析，或给出绝对路径。它不接受参数，也不经过 shell，因此脚本需要 shebang 与执行位。每次调用都会启动新的宿主进程，并有较短的超时；无响应的宿主不会阻塞启动器。

### 常驻宿主

`resident = true` 让同一个宿主进程跨调用保留。此时每次调用约 1 毫秒，而重新启动进程需要数毫秒。启动器关闭、宿主空闲两分钟，或某次调用崩溃或无响应时，进程会被丢弃；下一次调用再启动新的。修改配置的脚本也会在下次调用时生效。

无论是否常驻，宿主都必须做到每次调用无状态，但可以在自己的进程生命周期内缓存。

### 图标与预览

插件的标识 `icon` 与动作的 `icon` 必须是绝对路径：宿主自带的文件（`wayrun.script_dir()` 可定位脚本目录），或 Lua SDK 的 `wayrun.icon()` 解析出的主题图标。

结果自己的 `icon` 也可以是 `https://` URL。后端会在后台获取它并存入图标缓存，再把本地路径交给界面层，因此结果行先立即绘制，图片到位后再补上。

无法解析的主题名、无法解析的 `papirus:` 规范，或 `builtin:` 字形，都视为没有图标。结果没有图标时，启动器使用插件的标识图标；标识图标也缺失时，绘制内置占位图。结果行可以设置 `icon_shape = "circle"` 把图标框裁成圆形，头像需要这样做。

插件需要其他网络数据（API 缓存、非图片文件）时，仍由插件自己获取和缓存。结果图标与预览图由后端负责。

### 错误

宿主的 `search` 错误显示为一条仅显示的结果行，摘要取错误信息的第一行。`top` 出错时保留身份卡片。

### 语言

宿主通过 `WAYRUN_LOCALE` 继承启动器解析出的界面语言，也就是它自己文案表使用的语言标识（`en`、`zh_cn` 等）。宿主自己负责文案表与回退，启动器从不翻译插件文字。Lua 的写法见 [lua.md](lua.md#本地化)。

### API 版本

每个插件都会声明自己面向的 API 版本：宿主在 `list_plugins` 返回的标识信息里声明，Lua 脚本在插件表里声明。版本与启动器不一致时，启动器记一条日志，但仍会加载插件，插件可能仍然可用。

只有当插件能观察到变化时，这个数字才会变：绑定、签名、字段或沙箱规则。新增功能不会改变它，例如增加 `wayrun.crypto` 不影响版本；而 `wayrun.env` 增加一个可读范围会改变它。版本机制之前写的插件视为 `0`，在声明版本之前会一直收到警告。Lua 可以在运行时通过 `wayrun.api` 读取当前版本。

## 网络与代理

后端自己不发起任何外部请求，需要联网的已注册宿主会发起，例如 WayRun-Plugins 的 `web.lua`。它的 [`wayrun.http.get`](lua.md#api) 读取启动器进程的环境变量，因此在环境里或 systemd 单元的 `Environment=` 中设置代理：

```ini
# HTTP CONNECT 代理；user:password@ 可选
Environment=https_proxy=http://127.0.0.1:7890
Environment=http_proxy=http://127.0.0.1:7890
```

然后执行 `systemctl --user daemon-reload && systemctl --user restart wayrun-launcher`。

启动器按 `ALL_PROXY`、`HTTPS_PROXY`、`HTTP_PROXY` 的顺序读取，大小写均可。第一个可用的值生效，不可用的值会被跳过，因此坏掉的 `https_proxy` 不会遮住可用的 `http_proxy`。

`NO_PROXY` 按请求生效，支持精确、后缀与通配符。代理值要写全协议前缀：`http://` 表示 80 端口的 HTTP CONNECT 代理，`https://` 表示 443 端口。不写协议时按 HTTP 处理，而不是 SOCKS；`socks5://` 不在已发布的构建里。没有可用值时，请求直连。

## 编写宿主

宿主也可以手写。[WayRun-Plugins](https://github.com/Prslc/WayRun-Plugins) 工作区提供 Python 框架、示例与模板（`template/`、`template.lua`）。

Lua 插件属于第一方：二进制自己承载脚本，见 [lua.md](lua.md)。宿主协议是 [jsonrpc.md](jsonrpc.md) 记录的那部分 JSON-RPC 子集：`search`、`top`、 `list_plugins`。
