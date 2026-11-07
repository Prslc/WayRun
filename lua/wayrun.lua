---@meta

---@class WayrunStat
---@field mtime_ns integer
---@field size integer

---@class WayrunHTTPReply
---@field status integer
---@field headers table<string, string>
---@field body string
---@field stale boolean?

---@class WayrunFuzzyHit
---@field index integer
---@field kind string

---@class WayrunKVRecord
---@field key string
---@field value string

--- The `type` an action carries; the fields below say which variant uses what.
---@alias WayrunActionType
---| "run"
---| "run_in_terminal"
---| "launch"
---| "copy"
---| "desktop_action"
---| "reveal"
---| "terminal"
---| "open"
---| "open_with"
---| "plugin"

---@class WayrunAction
---@field type WayrunActionType
---@field cmd string?          run, run_in_terminal
---@field desktop_id string?   launch, desktop_action, open_with
---@field action_id string?    desktop_action
---@field text string?         copy
---@field uri string?          reveal, terminal, open, open_with
---@field plugin string?       plugin: the owning plugin id
---@field action string?       plugin: the command it runs
---@field payload any?         plugin: whatever the command interprets

--- A panel entry; `children` opens as a further level.
---@class WayrunActionItem
---@field title string
---@field action WayrunAction?
---@field children WayrunActionItem[]?
---@field icon string?
---@field id string?
---@field plugin string?
---@field default boolean?

--- The selected row's split-view payload.
---@class WayrunPreview
---@field type "image"|"text"
---@field path string?   image: a file the script ships, or an https:// URL
---@field text string?   text: at most 64 KiB
---@field format "plain"|"markdown"?   text: render as plain text (default) or Markdown

--- One result row a `search` or `top` returns.
---@class WayrunRow
---@field title string
---@field summary string?
---@field on_click WayrunAction?
---@field icon string?
---@field icon_shape "square"|"circle"?
---@field preview WayrunPreview?
---@field actions WayrunActionItem[]?

--- One plugin table a script returns.
---@class WayrunPlugin
---@field id string
---@field name string?
---@field icon string?
---@field description string?
---@field api integer?
---@field env string[]?
---@field exec string[]?
---@field read string[]?
---@field search fun(text: string, emit: fun(rows: WayrunRow[])?): WayrunRow[]?
---@field top fun(): WayrunRow[]?
---@field command fun(action: string, payload: any): any

---@class Wayrun
wayrun = {}

---@type integer
wayrun.api = 1

---@class WayrunFS
wayrun.fs = {}

---@class WayrunJSON
wayrun.json = {}

---@class WayrunTOML
wayrun.toml = {}

---@class WayrunHTTP
wayrun.http = {}

---@class WayrunCrypto
wayrun.crypto = {}

---@class WayrunSQLite
wayrun.sqlite = {}

---@class WayrunKV
wayrun.kv = {}

---@class WayrunFuzzy
wayrun.fuzzy = {}

---@return string?
function wayrun.home() end

---@return string?
function wayrun.cache_dir() end

---@param spec string
---@return string?
function wayrun.icon(spec) end

---@param text string
---@return string
function wayrun.urlencode(text) end

---@param path string
---@return string?
function wayrun.file_uri(path) end

---@param message string
function wayrun.log(message) end

---@return integer
function wayrun.time() end

--- The UTC civil-date fields for a Unix timestamp.
---@class WayrunDate
---@field year integer
---@field month integer   1-12
---@field day integer     1-31
---@field hour integer    0-23
---@field minute integer
---@field second integer

---@param seconds integer?   Unix seconds; defaults to now
---@return WayrunDate
function wayrun.date(seconds) end

---@return string
function wayrun.locale() end

---@param name string
---@return string?
function wayrun.env(name) end

---@param name string
---@return string?
function wayrun.which(name) end

---@class WayrunExecResult
---@field code integer
---@field stdout string
---@field stderr string
---@field stdout_truncated boolean  the 1 MiB cap cut the stream
---@field stderr_truncated boolean  the 8 KiB cap cut the stream

---@param program string
---@param args string[]?
---@return WayrunExecResult
function wayrun.exec(program, args) end

---@return string?
function wayrun.script_dir() end

---@param id string
---@return string?
function wayrun.plugin_dir(id) end

---@param id string
---@return string?
function wayrun.plugin_cache_dir(id) end

---@param name string
---@return any
function wayrun.import(name) end

---@param name string
---@return string?
function wayrun.fs.read(name) end

---@param path string
---@param data string
---@return boolean
function wayrun.fs.write(path, data) end

---@param path string
---@return boolean
function wayrun.fs.delete(path) end

---@param dir string
---@return string[]?
function wayrun.fs.list(dir) end

---@param path string
---@return WayrunStat?
function wayrun.fs.stat(path) end

---@param text string
---@return any
function wayrun.json.decode(text) end

---@param value any
---@return string
function wayrun.json.encode(value) end

---@param text string
---@return any
function wayrun.toml.decode(text) end

---@param url string
---@param params table?
---@param options table|number?
---@return WayrunHTTPReply
function wayrun.http.get(url, params, options) end

---@param requests table
---@return table[]
function wayrun.http.get_many(requests) end

---@param url string
---@param params table?
---@param options table|number?
---@return WayrunHTTPReply|nil
function wayrun.http.cached(url, params, options) end

---@param url string
---@param params table?
---@param options table|number?
---@return WayrunHTTPReply
function wayrun.http.post(url, params, options) end

---@param text string
---@return string
function wayrun.crypto.sha256(text) end

---@param text string
---@return string
function wayrun.crypto.md5(text) end

---@param key string
---@param text string
---@return string
function wayrun.crypto.hmac_sha256(key, text) end

---@param data string
---@return string
function wayrun.crypto.base64_encode(data) end

---@param text string
---@return string?
function wayrun.crypto.base64_decode(text) end

---@param path string
---@return integer
function wayrun.sqlite.snapshot(path) end

---@param handle integer
---@param sql string
---@param params table?
---@return table[]
function wayrun.sqlite.query(handle, sql, params) end

---@param key string
---@return string?
function wayrun.kv.get(key) end

---@param key string
---@param value string
---@param ttl_secs integer?
---@return boolean
function wayrun.kv.set(key, value, ttl_secs) end

---@param key string
---@return boolean
function wayrun.kv.delete(key) end

---@param prefix string?
---@return string[]
function wayrun.kv.keys(prefix) end

---@param prefix string?
---@return WayrunKVRecord[]
function wayrun.kv.pairs(prefix) end

---@param query string
---@param candidates string[]
---@return WayrunFuzzyHit[]
function wayrun.fuzzy.match(query, candidates) end
