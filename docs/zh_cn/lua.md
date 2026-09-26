# Lua 插件

一个插件可以是一个 Lua 脚本，由启动器二进制自身承载：
`wayrun --lua-host <script.lua>` 运行脚本，并应答与
[外部主机](plugins.md#外部主机)完全相同的 JSON-RPC 接口。核把 Lua 与 Python
插件一视同仁——Lua 只是启动更快、且不需要用户安装解释器。

## 编写脚本

脚本返回一组插件表，每个表即一个插件：

```lua
return {
  {
    id = "demo",                          -- 必须与 plugins.toml 条目一致
    name = "Demo",                        -- 显示在帮助卡片中
    icon = wayrun.icon("builtin:globe"),  -- 绝对路径，或 nil
    description = "Say hello",            -- 可选，显示在帮助卡片中
    search = function(text)               -- 路由查询的候选行，按序返回
      if text == "" then return {} end
      return {
        { title = "hello " .. text, summary = "from demo.lua" },
      }
    end,
    top = function() end,                 -- 可选：空查询视图
    forget = function(action) end,        -- 可选：认领一行以从历史移除
  },
}
```

行即线路协议中的[结果项](jsonrpc.md#结果项)：`title`、`summary`、`on_click`
（一个动作）、`icon`、`ephemeral`、`actions`、`badge`。不需要构造器——写成
线路形状的表即可，例如 `on_click = { type = "open", uri = "https://example.com" }`。

## `wayrun` 表

| 调用 | 作用 |
| --- | --- |
| `wayrun.home()` | `$HOME`，否则 nil |
| `wayrun.cache_dir()` | 启动器的缓存目录，否则 nil |
| `wayrun.icon(spec)` | 把 `builtin:…`、主题图标名或 `papirus:…` 解析为绝对路径；未命中为 nil |
| `wayrun.urlencode(text)` | 按 URL 需要做百分号编码 |
| `wayrun.t(key, args)` | 取启动器自带文案表中的一条，`%{name}` 由 `args` 填充 |
| `wayrun.log(message)` | 以脚本名义写入启动器的 journal |
| `wayrun.web_search_engine()` | 配置的搜索引擎，如 `"google"` |
| `wayrun.json.decode(text)` / `wayrun.json.encode(value)` | JSON 进出 |
| `wayrun.fs.list(dir)` | `dir` 下的条目名，否则 nil |
| `wayrun.fs.stat(path)` | `{ mtime_ns, size }`，否则 nil |
| `wayrun.http.get(url, params?, timeout_ms?)` | 阻塞式 GET；`params` 追加查询参数 |
| `wayrun.sqlite.snapshot(path)` | 打开一份 SQLite 文件的不可变副本，返回句柄 |
| `wayrun.sqlite.query(handle, sql, params?)` | 行以表返回；NULL 列读作缺失 |

## 沙箱

`os`、`io`、`package`、`load` 与 `print` 均不可达：脚本读不了任意文件、起不了
进程；它需要的能力由上表提供。调用中抛错则本次返回空行并记录到 journal，因此
`wayrun.log` 与对风险操作的 `pcall` 就是调试手段。

## 限制

- Lua 插件必须有非空 keyword：它只应答自己的路由查询。默认链（keyword 为
  `""`）保留给内置插件。
- 行序即脚本返回的顺序；没有相关度通道。
- 图标必须是绝对路径；用 `wayrun.icon` 获取。

## 注册

与任何主机一致：单个可执行文件 token，因此脚本需要 shebang 与执行位——随二进
制分发的脚本则用 `script` 键。

```toml
[[plugins]]
id = "demo"
keyword = "dm"
command = "/home/you/.config/wayrun/demo.lua"
resident = true   # 跨调用保留一个主机进程
```

脚本首行必须是 `#!/usr/bin/env -S wayrun --lua-host`。`core/assets/lua/` 下的
`firefox.lua` 与 `web.lua` 是两个完整示例，前者读 `places.sqlite`，后者读网页。
