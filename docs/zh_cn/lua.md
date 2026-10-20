# Lua 插件

插件也可以就是一个 Lua 脚本：无框架、无需安装 Python。WayRun 二进制自身承载它
（`wayrun --lua-host`），与 Python 主机共用同一套 JSON-RPC 接口，启动只需几毫秒；
启动器的能力都通过一张 `wayrun` 表暴露。

## 编写脚本

脚本返回一组插件表，每个表就是一个插件：

```lua
#!/usr/bin/env -S wayrun --lua-host
return {
  {
    id = "my-plugin",                    -- 必须与 plugins.toml 条目一致
    name = "My Plugin",
    -- icon = wayrun.script_dir() .. "/icon.svg", -- 自带的图标文件，放在脚本旁边
    description = "简短描述",
    api = 1,                             -- 编写时所针对的插件契约版本
    env = { "MY_TOKEN" },                -- 可读的环境变量名
    search = function(text, emit)        -- 路由查询的候选行，按序返回
      return {
        { title = "You searched: " .. text,
          on_click = { type = "copy", text = text } },
      }
    end,
    top = function() end,                -- 可选：空查询视图
  },
}
```

行就是 [jsonrpc.md](jsonrpc.md#结果项) 记录的普通表，字段有 `title`、`summary`、
`on_click`、`icon`、`icon_shape`、`preview`、`actions`，不需要构造器。例如
`on_click = { type = "open", uri = "https://example.com" }`。
`preview = { type = "image", path = "/absolute/file.png" }`（或由 core 缓存的
`https://` URL）会在选中行旁边打开分栏预览；当前版本解码 PNG、SVG、JPEG、WebP、
GIF 与 BMP。
`preview = { type = "text", text = "…" }` 在同一列显示按宽度换行的纯文本摘要。

流式搜索还会收到 `emit`：后端支持流式时传入一个函数，每次 `emit(rows)` 立刻显示
这批行，搜索仍在继续；最终返回值仍是权威结果。旧后端传 `nil`，因此写成
`if emit then … end`，此时插件与以前一样只答复一次。

## 插件表

`id` 必填，且必须与 `plugins.toml` 条目一致；`name`、`icon`、`description` 是启动器
展示的身份，形状见
[jsonrpc.md](jsonrpc.md#外部主机--后端身份发现)，图标规则见
[plugins.md](plugins.md#外部主机)。`search` 应答路由查询，`top` 应答空查询。
`api` 声明脚本编写时所针对的插件契约版本，版本号的变动规则见
[plugins.md](plugins.md)；`wayrun.api` 读取当前启动器所支持的版本，因此一份脚本
可以同时兼容两个版本。

`env` 列出插件可读的环境变量名，必须逐个写全，不支持通配。读到名单之外的名字会
直接报错；声明了但进程里没有设置的变量才返回 nil。`env` 只能在 `search` 与 `top`
这些插件调用里读，在脚本加载（顶层代码）时读会报错。要在脚本里拿 home 或缓存目录，
用 `wayrun.home()` 与 `wayrun.cache_dir()`，不必读环境。

`read` 列出插件可以读取的区域：绝对路径或 `~/…`，同样逐个写全。每次读取都会解析
符号链接，区域内的链接不能把路径带到区域外；`fs.list`、`fs.stat` 与
`sqlite.snapshot` 遇到区域之外的路径直接报错，而插件自己的目录与脚本所在目录始终
算在区域内。

`exec` 列出插件可通过 `wayrun.exec` 运行的程序名：必须在 `PATH` 上，不接受带斜杠的
路径。子进程在 `$HOME` 下启动，只保留会话的 `PATH`、`HOME`、XDG、D-Bus 与 Wayland
变量，加上插件在 `env` 里声明过的名字；启动器环境中的其他变量不会传入。

## 动作（Action）

`on_click` 与行上 `actions` 里的每一项都是命令：带 `type` 标签的普通表，形状见
[jsonrpc.md](jsonrpc.md#动作)。行的 `actions` 组成 `Shift+Enter` 动作面板；启动器
在它们之外补了什么，见 [plugins.md](plugins.md#结果动作)。

```lua
actions = {
  {
    title = "Copy URL",
    id = "copy_url",                    -- 有了它，用户可把该项设为该行的默认动作
    action = { type = "copy", text = uri },
    icon = wayrun.script_dir() .. "/copy.svg", -- 自带的图标文件，与所有图标一样
  },
},
```

没有 `on_click` 的行是纯展示行；它照样可以带 `actions`。

## `wayrun` 表

| 调用 | 作用 |
| --- | --- |
| `wayrun.api` | 当前启动器所支持的插件契约版本，供跨版本脚本分支 |
| `wayrun.home()` | `$HOME`，否则 nil |
| `wayrun.cache_dir()` | 启动器的缓存目录，否则 nil |
| `wayrun.icon(name)` | 把主题图标名或 `papirus:…` 规范解析为绝对路径（绝对路径原样通过）；未命中或 `builtin:` 字形为 nil |
| `wayrun.urlencode(text)` | 按 URL 需要做百分号编码 |
| `wayrun.file_uri(path)` | 绝对路径对应的 `file://` URI，编码规则与内置行一致；相对路径返回 nil |
| `wayrun.log(message)` | 以脚本和调用插件的名义写入启动器的 journal |
| `wayrun.time()` | Unix 秒，用于签名与缓存 TTL |
| `wayrun.locale()` | 启动器解析出的界面语言，也就是它自己文案表使用的语言标识（`en`、`zh_cn`…） |
| `wayrun.env(name)` | 插件在 `env` 中声明的变量；未设置则为 nil；未声明的名字抛出错误 |
| `wayrun.which(name)` | 启动器 `$PATH` 上某个可执行文件的路径，否则 nil；与 `run` 行的命令将看到的 PATH 相同 |
| `wayrun.exec(program, args?)` | 以 argv（不经过 shell）在 `$HOME` 下运行插件声明过的 `PATH` 程序，1.5 秒超时；返回 `{code, stdout, stderr}`，输出上限 64 KiB / 8 KiB；未声明或超时会报错 |
| `wayrun.script_dir()` | 脚本自身所在目录，用于携带图标等文件 |
| `wayrun.import(name)` | 在同一个沙箱里加载 `<script_dir>/<name>.lua` 并缓存其值；返回 nil 时缓存为 true；循环或越界 import 会报错 |
| `wayrun.plugin_dir(id)` | 插件可读目录：`$XDG_CONFIG_HOME/wayrun/plugins/<id>` |
| `wayrun.plugin_cache_dir(id)` | 插件可写缓存目录：`$XDG_CACHE_HOME/wayrun/plugins/<id>` |
| `wayrun.fs.read(name)` | 读取文件：相对名在插件目录下，绝对/`~/` 路径须在声明区域、插件目录、缓存目录或脚本目录内；缺失返回 nil，越界报错 |
| `wayrun.toml.decode(text)` | TOML 进，表出 |
| `wayrun.json.decode(text)` / `wayrun.json.encode(value)` | JSON 进出 |
| `wayrun.fs.list(dir)` | 上述位置中某目录下按名称排序的条目名，否则 nil |
| `wayrun.fs.stat(path)` | 上述位置中路径的 `{ mtime_ns, size }`，否则 nil |
| `wayrun.fs.write(path, data)` | 把字符串写入插件目录或缓存目录（自动创建父目录）；成功返回 true；`kv.db` 及其兄弟文件为 `kv` 保留 |
| `wayrun.fs.delete(path)` | 删除插件目录或缓存目录中的文件；文件本就不存在时也返回 true |
| `wayrun.http.get(url, params?, options?)` | 阻塞式 GET，返回 `{status, headers, body}`；传输错误会报错；`params` 追加查询参数，其保留键 `headers` 为请求头表；`options` 是带 `timeout_ms`、`headers`、`ttl` 的表，或直接给一个毫秒数作为超时 |
| `wayrun.http.get_many(requests)` | 并发 GET：`requests` 是 `{url, params?, options?}` 的数组，按顺序返回；传输失败的那一项为 `{error = "…"}` |
| `wayrun.http.cached(url, params?, options?)` | 不访问网络，读取已存回复；没有则为 nil；超过 `ttl` 的回复带 `stale = true` |
| `wayrun.http.post(url, params?, options?)` | 同上，发送一个 body：`json = value`、`form = {…}` 或 `body = "…"` 三选一；从不缓存 |
| `wayrun.crypto.sha256(text)` / `wayrun.crypto.md5(text)` | 小写十六进制摘要 |
| `wayrun.crypto.hmac_sha256(key, text)` | 小写十六进制 HMAC |
| `wayrun.crypto.base64_encode(data)` / `wayrun.crypto.base64_decode(text)` | 对原始字节做 base64 编解码；解码遇到非法输入返回 nil |
| `wayrun.sqlite.snapshot(path)` | 打开一份声明区域内 SQLite 文件的不可变副本，返回句柄；同一文件返回同一句柄，句柄跟随其最新副本 |
| `wayrun.sqlite.query(handle, sql, params?)` | 行以表返回；NULL 列读作缺失；查询也受区域限制：句柄的来源文件必须在调用插件的声明区域内 |
| `wayrun.kv.get(key)` | 取回存下的字符串；缺失或已过期则为 nil |
| `wayrun.kv.set(key, value, ttl_secs?)` | 在 `key` 下存一个字符串；给了 ttl 会过期；底层是插件自己目录里的一个 sqlite 数据库 |
| `wayrun.kv.delete(key)` | 删掉一个键 |
| `wayrun.kv.keys(prefix?)` / `wayrun.kv.pairs(prefix?)` | 前缀下未过期的键（或 `{key, value}` 记录），按键名排序 |
| `wayrun.fuzzy.match(query, candidates)` | 对字符串数组返回 `{index, kind}` 命中，最优在前；kind 就是启动器自己的匹配词汇（exact、prefix、word、substring、loose） |

`http.get` 带 `ttl` 时，重复调用由插件自己的存储应答，不再走网络；它只缓存 2xx 的
文本响应，这正是“每次按键一次查询”能负担得起的原因。返回的 body 是原始字节，响应头
名称小写；超时默认两秒，并且不会超过主机调用本身的上限，因此慢端点会抛出可被
`pcall` 捕获的错误，而不是让主机被强制结束。

`http.get_many` 最多同时发四个请求，每项先查自己的 `ttl` 缓存；返回顺序与输入
一致，传输失败的那一项是 `{error = "…"}`，不会让整批报错。整批共享主机调用的
时限，单请求超时应控制在这个范围内。

`http.cached` 只读插件自己的存储，过期的条目也会返回，并用 `stale = true` 标记。
这正是 `emit` 的用法：先显示过期行，再调用 `http.get` 并把新结果返回。

## 沙箱

`os`、`io`、`package` 与 `load` 均不可达：脚本起不了进程。读取受路径限制：相对名
落在插件自己的目录（`$XDG_CONFIG_HOME/wayrun/plugins/<id>/`）下，绝对或 `~/` 路径
必须位于声明区域、插件目录、缓存目录或脚本目录内；`sqlite.snapshot` 用同一规则。写入范围限于
插件自己的目录或缓存目录（自动创建父目录）：相对名落在插件目录；配置目录里的
`kv.db` 及其 `-wal`/`-shm`/`-journal` 兄弟文件保留给 `kv`。`fs.delete` 可以从这
两处删除文件。

`wayrun.env` 只能读当前插件在调用中声明的名字，`http` 可访问网络。`print` 与
`wayrun.log` 写入启动器的 journal，绝不写 stdout（stdout 是 JSON-RPC 通道），且每次
调用都有上限。沙箱约束脚本能做什么；能读到什么，由插件表的 `env` 与 `read` 声明。
`search` 抛错时，本次返回一条带错误信息第一行的展示行；`top` 失败则保留身份卡片。
`print`、`wayrun.log` 与 `pcall` 仍是调试手段。

## 调试

不进启动器，直接驱动脚本并读一行回复：

```sh
printf '{"jsonrpc":"2.0","method":"search","params":{"plugin":"<id>","text":"…"},"id":1}\n' \
  | wayrun --lua-host plugin.lua
```

缺少 `env` 或 `read` 声明时，错误会直接点名，把它加进插件表即可，不必反复试探。

`wayrun plugin check <script.lua>` 按主机的加载方式跑一遍脚本，报告会让它跑不起来的
地方：shebang 与执行位、脚本返回的插件表、未知的键、比启动器新的 `api`、缺失的
`search`/`top` 或身份图标，以及 `plugins.toml` 是否登记了脚本声明的 id。有问题时以
非零状态退出，其余只是警告。

## 模块

`wayrun.import("lib/util")` 会在同一个沙箱环境里加载 `<script_dir>/lib/util.lua`
（`.lua` 可省略）并返回其值；之后对同一规范路径的 import 返回缓存值。返回 nil 时按
`require` 的习惯缓存为 true。名字必须是相对路径，不能用 `..` 跳出脚本目录；指向
目录外的符号链接也会被拒绝。循环 import 直接报错，而不是递归。

常驻主机还会监视脚本 import 过的每个文件：任一文件变化时，会在下一次请求前重载
沙箱；重载失败则保留上一版可用状态，等下次变化再试。

## 本地化

`wayrun.locale()` 返回启动器解析出的界面语言，也就是它自己文案表使用的语言标识
（`en`、`zh_cn` 等）；脚本加载时就能读到，所以插件表的 `name` 与 `description` 也能
跟着切换。启动器会把所有 `zh_*` 变体归并为 `zh_cn`；其他语言以归一化后的形式出现
（如 `fr_fr`）。界面语言在启动时读取一次，切换语言和启动器本身一样需要重启。

文案表与回退由插件自己负责，启动器不会翻译插件文本。把文案放在以界面语言为键的表
里，并回退到你必定附带的那一种：

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

要翻译的是插件自己写的文案：身份名称与描述、行标题与摘要、动作标题、空状态与错误
提示。不要翻译数据：仓库名、文件名、命令、URI 与剪贴板文本都不是翻译对象。文案表
较大时，每种语言放一个 JSON 文件在脚本同目录，用 `wayrun.fs.read` 读入，再交给
`wayrun.json.decode`：

```lua
local dir = wayrun.script_dir() .. "/locales/"
local text = wayrun.fs.read(dir .. wayrun.locale() .. ".json")
  or wayrun.fs.read(dir .. "en.json")
local strings = wayrun.json.decode(text or "{}")
```

其他语言的主机从 `$WAYRUN_LOCALE` 读取同一个值。

## 限制

- Lua 插件可以用 `keyword = ""` 加入默认链。它的行是列出型而不是计分型，因此排在
  所有计分型行之后，只填补剩余槽位。默认链会给没有关键词的主机一个很短的期限，
  超时就跳过本次；因此 `search` 要足够轻，查询不相关时尽快返回空。
- 行序即脚本返回的顺序；没有相关度通道。
- 图标遵循 [plugins.md](plugins.md#外部主机) 的规则：脚本自带的文件、
  `wayrun.icon(name)` 解析出的主题图标，或 `https://` URL 形式的远程图片——core 会在
  后台抓取并缓存。同一个 URL 也可以作为图片预览的 `path`；头像加
  `icon_shape = "circle"`。其他下载数据用 `wayrun.plugin_cache_dir(id)` 缓存，
  用 `wayrun.fs.delete` 清理。

## 注册

复制 `template.lua`，保留 shebang 与执行位（`chmod +x`），像任何主机一样注册：单个
可执行文件名，字段见 [plugins.md](plugins.md#条目字段)，契约见
[jsonrpc.md](jsonrpc.md)。脚本首行必须是 `#!/usr/bin/env -S wayrun --lua-host`。

编辑器补全：把 [`lua/wayrun.lua`](../../lua/wayrun.lua) 加入 LuaLS 的
`Lua.workspace.library`，它声明了同一张 `wayrun` 表。

[firefox.lua](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/firefox.lua)
与 [web.lua](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/web.lua)
是两个完整示例，分别读取 `places.sqlite` 与网页。
