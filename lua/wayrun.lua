---@meta

---@class WayrunStat
---@field mtime_ns integer
---@field size integer

---@class WayrunHTTPReply
---@field status integer
---@field headers table<string, string>
---@field body string

---@class WayrunFuzzyHit
---@field index integer
---@field kind string

---@class WayrunKVRecord
---@field key string
---@field value string

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
