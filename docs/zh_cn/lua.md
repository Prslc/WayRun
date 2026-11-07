# Lua 插件

插件也可以只是一个 Lua 脚本。不需要框架，也不需要安装 Python：WayRun 二进制自己承载脚本（`wayrun --lua-host`），应答与 Python 宿主相同的 JSON-RPC 接口。它能在毫秒级启动，启动器的插件 API 就是一个名为 `wayrun` 的全局表。

## 编写脚本

脚本返回一组插件表，每个表是一个插件：

```lua
#!/usr/bin/env -S wayrun --lua-host
return {
  {
    id = "my-plugin",                               -- 必须与 plugins.toml 的 id 一致
    name = "My Plugin",
    icon = wayrun.script_dir() .. "/icon.svg",      -- 随脚本发布的图标文件
    description = "Short description",
    api = 1,                                        -- 脚本面向的插件契约
    env = { "MY_TOKEN" },                           -- 允许读取的环境变量名
    search = function(text, emit)                   -- 路由查询的结果行，按顺序
      return {
        { title = "You searched: " .. text,
          on_click = { type = "copy", text = text } },
      }
    end,
    top = function() end,                           -- 可选：空查询视图
    command = function(action, payload) end,        -- 可选：结果行的插件动作
  },
}
```

结果行是普通表，结构见 [jsonrpc.md](jsonrpc.md#结果项)：`title`、`summary`、 `on_click`、`icon`、`icon_shape`、`preview` 与 `actions`。不需要构造器：

```lua
on_click = { type = "open", uri = "https://example.com" }
```

`preview = { type = "image", path = "/absolute/file.png" }` 在列表旁为选中行打开分栏预览。路径可以是 `https://` URL，由后端缓存。当前版本支持 PNG、SVG、JPEG、 WebP、GIF 与 BMP。

`preview = { type = "text", text = "…" }` 在同一栏放一段自动换行的纯文本。加上 `format = "markdown"` 会渲染标题、强调、代码、列表与引用；旧版启动器忽略该字段，按纯文本显示原始内容。

流式搜索还会收到 `emit`。后端支持流式时传入一个函数，每次 `emit(rows)` 立即显示这批结果，搜索继续运行。最终返回值仍然有效。旧版后端传入 `nil`，用 `if emit then … end` 判断；这类后端不会发送能力标记，插件仍然只应答一次。

## 插件表

### 身份与契约

`id` 必填，且必须与 `plugins.toml` 的条目一致。`name`、`icon` 与 `description` 是启动器显示的身份信息，结构见 [jsonrpc.md](jsonrpc.md#外部宿主到后端-身份发现)，图标规则见 [plugins.md](plugins.md#外部宿主)。

`search` 应答路由来的查询，`top` 应答空查询。`api` 声明脚本面向的插件契约，版本规则见 [plugins.md](plugins.md)。`wayrun.api` 读取当前启动器支持的版本，因此同一份脚本可以跨两个版本使用。

### env

`env` 列出插件可以读取的环境变量名，必须精确，不支持模式匹配。未声明的名称会报错；已声明但未设置的值为 nil。读取只在 `search` 与 `top` 中可用，脚本加载时不可用。脚本原本想从环境变量取得的路径，请改用 `wayrun.home()` 与 `wayrun.cache_dir()`。

### read

`read` 列出插件可以读取的区域：绝对路径或 `~/…`，必须精确，不支持模式匹配。每次读取都会解析符号链接，因此区域内的链接无法指向区域外。`fs.list`、`fs.stat` 与 `sqlite.snapshot` 访问区域外内容时会报错。插件自己的目录与脚本所在目录始终在可读范围内。

### exec

`exec` 列出插件可以用 `wayrun.exec` 运行的程序名：`PATH` 中的名字，不含斜杠。子进程在 `$HOME` 启动，保留会话的 `PATH`、`HOME`、XDG、D-Bus 与 Wayland 变量，以及插件 `env` 声明的名称；启动器环境中的其他变量不会传入。

### command

`command` 接收结果行 `{ type = "plugin", … }` 动作的 `action` 与 `payload`（见[动作](#动作)）。它在插件自己的作用域中运行，因此 `fs.write`、`env` 与 `exec` 与搜索时一致。此时启动器已经在关闭：失败只写入日志，不显示在列表里；返回值被忽略。

## 动作

`on_click` 与结果行 `actions` 的每一项都是命令：按 `type` 标记的普通表，结构见 [jsonrpc.md](jsonrpc.md#动作)。结果行的 `actions` 组成 `Shift+Enter` 动作面板；启动器在它们周围补什么，见 [plugins.md](plugins.md#结果动作)。

```lua
actions = {
  {
    title = "Copy URL",
    id = "copy_url",                    -- 允许用户把它设为该行的默认动作
    action = { type = "copy", text = uri },
    icon = wayrun.script_dir() .. "/copy.svg", -- 随脚本发布的图标，与其他图标相同
  },
},
```

没有 `on_click` 的结果行只用于显示，仍然可以携带 `actions`。

`{ type = "plugin", … }` 动作用于提交只有插件自己能完成的事。启动器不执行它，而是交回脚本的 `command` 处理：

```lua
on_click = { type = "plugin", plugin = "my-plugin", action = "new",
             payload = { title = "Shopping list" } },
-- command = function(action, payload) ... end 写入文件，不需要 shell
```

## API

启动器把插件 API 作为全局表 `wayrun` 注入沙箱。下表按用途分组列出各项调用。

### 身份与环境

| 调用 | 作用 |
| --- | --- |
| `wayrun.api` | 当前启动器支持的插件契约，供跨两个版本的脚本判断 |
| `wayrun.home()` | `$HOME`，没有则为 nil |
| `wayrun.cache_dir()` | 启动器的缓存目录，没有则为 nil |
| `wayrun.locale()` | 启动器解析出的界面语言，即它自己文案表使用的语言标识（`en`、`zh_cn` 等） |
| `wayrun.env(name)` | 插件在 `env` 中声明的变量；未设置时为 nil，未声明会报错 |
| `wayrun.which(name)` | 可执行文件在启动器 `$PATH` 中的路径，没有则为 nil；与 `run` 结果行的命令看到的 PATH 一致 |
| `wayrun.script_dir()` | 脚本自己的目录，用于随脚本发布的图标 |
| `wayrun.plugin_dir(id)` | 插件可读的目录，`$XDG_CONFIG_HOME/wayrun/plugins/<id>` |
| `wayrun.plugin_cache_dir(id)` | 插件的可写缓存，`$XDG_CACHE_HOME/wayrun/plugins/<id>` |

### 文本、时间与匹配

| 调用 | 作用 |
| --- | --- |
| `wayrun.icon(name)` | 把主题图标名或 `papirus:…` 规范解析为绝对路径（绝对路径原样返回）；未命中或 `builtin:` 字形时为 nil |
| `wayrun.urlencode(text)` | 按 URL 规则做百分号编码 |
| `wayrun.file_uri(path)` | 绝对路径的 `file://` URI，编码方式与内置结果行一致；相对路径为 nil |
| `wayrun.log(message)` | 写入启动器日志，标注脚本与调用插件的名称 |
| `wayrun.time()` | Unix 秒，可用于签名与缓存 TTL |
| `wayrun.date(seconds?)` | Unix 时间戳对应的 UTC 日期字段（`year`、`month`、`day`、`hour`、`minute`、`second`）；默认取当前时间，旧版启动器没有这个调用 |
| `wayrun.fuzzy.match(query, candidates)` | 在字符串数组中做模糊匹配，按匹配程度返回 `{index, kind}`；kind 与启动器一致（exact、prefix、word、substring、loose） |

### 文件与数据格式

| 调用 | 作用 |
| --- | --- |
| `wayrun.fs.read(name)` | 读取文件：插件目录下的相对名，或声明的读取区域、插件目录、插件缓存、脚本目录中的绝对路径或 `~/` 路径；不存在时为 nil，越界时报错 |
| `wayrun.fs.list(dir)` | 上述位置中某目录的条目名，已排序；没有则为 nil |
| `wayrun.fs.stat(path)` | 上述位置中路径的 `{ mtime_ns, size }`，没有则为 nil |
| `wayrun.fs.write(path, data)` | 把字符串写入插件目录或缓存（自动创建父目录）；成功为 true；`kv.db` 及其同级文件保留 |
| `wayrun.fs.delete(path)` | 删除插件目录或缓存中的文件；文件本来就不存在时也为 true |
| `wayrun.toml.decode(text)` | TOML 输入，表输出 |
| `wayrun.json.decode(text)` `wayrun.json.encode(value)` | JSON 输入与输出 |

### 进程与网络

| 调用 | 作用 |
| --- | --- |
| `wayrun.exec(program, args?)` | 在 `$HOME` 下用 argv 运行已声明的 `PATH` 程序（不经 shell），1.5 秒后超时；返回 `{code, stdout, stderr, stdout_truncated, stderr_truncated}`，超过上限（1 MiB/8 KiB）的流会被截断并标记；未声明的程序或超时会报错 |
| `wayrun.http.get(url, params?, options?)` | 阻塞式 GET，返回 `{status, headers, body}`；传输错误会报错；`params` 追加查询值，其中 `headers` 表是保留键；`options` 是含 `timeout_ms`、`headers`、`ttl` 的表，也可以直接给毫秒超时 |
| `wayrun.http.get_many(requests)` | 并发 GET：`requests` 是 `{url, params?, options?}` 数组，按顺序返回；某个请求传输失败时，该位置为 `{error = "…"}` |
| `wayrun.http.cached(url, params?, options?)` | 不发网络请求，直接返回已存应答；没有则为 nil；`ttl` 已过时 `stale` 为 true |
| `wayrun.http.post(url, params?, options?)` | 同上，但发送一个请求体：`json = value`、`form = {…}` 或 `body = "…"`；从不缓存 |

#### HTTP 缓存与批量请求

`http.get` 带 `ttl` 时，重复调用由插件自己的存储应答，不再走网络。只有 2xx 文本应答会被存储，这也是每次按键都发起查询仍然负担得起的原因。应答体以原始字节返回，响应头名称转成小写。超时默认两秒，并被限制在宿主调用自身的上限内，因此慢端点会让 `pcall` 拿到错误，而不是让宿主进程被终止。

`http.get_many` 最多同时发四个请求，并先检查各自的 `ttl` 缓存。应答保持输入顺序；某个请求传输失败时，它的位置是 `{error = "…"}`，不会让整批报错。整批共享宿主调用的上限，因此单个请求的超时要小于整批的上限。

`http.cached` 只从插件存储应答，过期条目也返回，并把过期的标记为 `stale = true`。配合 `emit` 使用：先显示旧结果，再调用 `http.get` 并返回新结果。

### 加密、SQLite 与键值存储

| 调用 | 作用 |
| --- | --- |
| `wayrun.crypto.sha256(text)` `wayrun.crypto.md5(text)` | 小写十六进制摘要 |
| `wayrun.crypto.hmac_sha256(key, text)` | 小写十六进制 HMAC |
| `wayrun.crypto.base64_encode(data)` `wayrun.crypto.base64_decode(text)` | 原始字节的 base64 编码与解码；解码失败返回 nil |
| `wayrun.sqlite.snapshot(path)` | 在声明区域内复制一份不可变的 SQLite 文件，打开并返回句柄；同一个文件对应同一个句柄，并总是指向最新的副本 |
| `wayrun.sqlite.query(handle, sql, params?)` | 结果行以表的形式返回；NULL 列读作缺失；句柄的源文件必须位于调用插件的区域内 |
| `wayrun.kv.get(key)` | 已存的字符串；不存在或已过期时为 nil |
| `wayrun.kv.set(key, value, ttl_secs?)` | 把字符串存到 `key` 下；带 ttl 时会过期；存储是插件目录中的 SQLite 数据库 |
| `wayrun.kv.delete(key)` | 删除一个键 |
| `wayrun.kv.keys(prefix?)` `wayrun.kv.pairs(prefix?)` | 某前缀下未过期的键，或 `{key, value}` 记录，按键排序 |

## 沙箱

### 禁止的内容

`os`、`io`、`package` 与 `load` 都不可达，脚本无法自行运行程序。需要运行程序时用 `wayrun.exec`，且程序必须在 `exec` 中声明。

### 读取

相对名称在插件自己的目录下解析（`$XDG_CONFIG_HOME/wayrun/plugins/<id>/`）。绝对路径或 `~/` 路径必须位于声明的读取区域、插件目录、插件缓存或脚本目录。 `sqlite.snapshot` 遵循同一规则。

### 写入

写入范围更窄。`fs.write` 只能写入插件自己的目录或缓存，并会创建父目录。相对名称落在配置目录，其中的 `kv.db` 及其同级的 `-wal`/`-shm`/`-journal` 文件保留。 `fs.delete` 可以从这两处删除文件。

### 环境与网络

`wayrun.env` 只读取调用插件声明的名称；`http` 可以访问网络。

### 日志与错误

`print` 与 `wayrun.log` 写入启动器日志，不写 stdout（stdout 用于 JSON-RPC），两者每次调用都有上限。

沙箱限定脚本能做什么；能读什么由插件的 `env` 与 `read` 声明。`search` 抛错时显示为一条错误行，内容取错误信息第一行；`top` 失败时保留身份卡片。`print`、 `wayrun.log`，以及用 `pcall` 包住风险操作，是主要调试手段。

## 模块

`wayrun.import("lib/util")` 在同一个沙箱环境中加载 `<script_dir>/lib/util.lua` （`.lua` 可省略）并返回其值。之后对同一规范路径的导入返回缓存值。返回 nil 时按 `true` 缓存，与 `require` 一致。

名称必须相对，不能用 `..` 越出目录。解析到脚本目录之外的符号链接会被拒绝。循环导入会报错，不会递归。

常驻宿主也会监听脚本导入的每个文件。修改其中一个文件后，下一次请求前会重新加载沙箱；加载失败时，上一份可用版本继续服务，直到下一次改动。

## 本地化

`wayrun.locale()` 返回启动器解析出的界面语言，也就是它自己文案表使用的语言标识，例如 `en` 或 `zh_cn`。脚本加载时就可以调用，因此插件表的 `name` 与 `description` 也能本地化。所有 `zh_*` 变体都已折叠为 `zh_cn`；其他语言会以规范化形式传入（`fr_fr`）。该值只在启动时读取，修改后需要和启动器一样重启。

插件自己负责文案表与回退，启动器从不翻译插件文字。把字符串放进按语言分键的表，并回退到始终随插件发布的那一份：

```lua
local CATALOGS = {
  en = { description = "Search GitHub repositories", copy_url = "Copy HTML URL" },
  zh_cn = { description = "搜索 GitHub 仓库", copy_url = "复制 HTML 链接" },
}

local fallback = CATALOGS.en
local strings = CATALOGS[wayrun.locale()] or fallback
local function t(key)
  return strings[key] or fallback[key]
end
```

翻译插件作者写的说明文字：插件名称与描述、结果行标题与摘要、动作标题、空状态与错误信息。数据不要翻译：仓库名、文件名、命令、URI 与剪贴板文字都不是文案。

文案较多时，在脚本旁为每种语言放一个 JSON 文件，用 `wayrun.fs.read` 读入 `wayrun.json.decode`：

```lua
local dir = wayrun.script_dir() .. "/locales/"
local text = wayrun.fs.read(dir .. wayrun.locale() .. ".json")
  or wayrun.fs.read(dir .. "en.json")
local strings = wayrun.json.decode(text or "{}")
```

其他语言的宿主从 `$WAYRUN_LOCALE` 读取同一个值。

## 限制

- Lua 插件可以用 `keyword = ""` 加入默认链。它的结果行只按顺序列出，不参与打分，因此排在所有已打分结果行之后，只填剩余位置。默认链给无关键词宿主四分之一秒的超时，一旦超出就跳过，因此 `search` 要尽量轻，不属于自己的查询直接返回空。
- 结果行保持脚本返回的顺序，没有相关度排序。
- 图标规则见 [plugins.md](plugins.md#外部宿主)：脚本自带的文件、`wayrun.icon(name)` 解析出的主题图标，或 `https://` URL 的远程图片（后端在后台获取并缓存）。同一个 URL 也可以作为图片预览的 `path`；头像可以设置 `icon_shape = "circle"`。其他下载数据用 `wayrun.plugin_cache_dir(id)` 存放，用 `wayrun.fs.delete` 清理。

## 注册

复制 `template.lua`，保留 shebang 与执行位（`chmod +x`），然后按同样的 `plugins.toml` 条目注册：只写可执行文件本身（不含参数），字段见 [plugins.md](plugins.md#条目字段)，契约见 [jsonrpc.md](jsonrpc.md)。脚本首行必须是 `#!/usr/bin/env -S wayrun --lua-host`。

`wayrun plugin add <script.lua> --keyword <k>` 会先加载脚本再写入条目，因此缺少 shebang 或执行位时会在注册前失败。

编辑器补全：把 [`lua/wayrun.lua`](https://github.com/Prslc/WayRun/blob/main/lua/wayrun.lua) 加入 LuaLS 的 `Lua.workspace.library`，它声明了同一套 `wayrun` API。

[firefox.lua](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/firefox.lua) 与 [web.lua](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/web.lua) 是完整的示例，分别读取 `places.sqlite` 与网页。

## 调试

不经过启动器，直接驱动脚本并读取一次应答：

```sh
printf '{"jsonrpc":"2.0","method":"search","params":{"plugin":"<id>","text":"…"},"id":1}\n' \
  | wayrun --lua-host plugin.lua
```

缺少 `env` 或 `read` 声明时，错误信息会点出名称，把它加进插件表即可，不要靠试探。

`wayrun plugin check <script.lua>` 按宿主的方式加载脚本，并报告可能导致它无法工作的内容：shebang 与执行位、返回的插件表、未知键、比启动器更新的 `api`、缺少 `search`/`top` 或标识图标，以及 `plugins.toml` 是否注册了它声明的 id。有问题会让命令失败，其余是警告。
