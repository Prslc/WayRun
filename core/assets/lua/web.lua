#!/usr/bin/env -S wayrun --lua-host
-- Web search suggestions from the configured engine's Firefox-style suggest
-- endpoint, one header row plus one row per suggestion.

local ENGINES = {
  google = {
    name = "Google",
    icon = "builtin:globe",
    search_url = "https://www.google.com/search?q=",
    suggest_url = "https://suggestqueries.google.com/complete/search?client=firefox",
  },
  duckduckgo = {
    name = "DuckDuckGo",
    icon = "builtin:globe",
    search_url = "https://duckduckgo.com/?q=",
    suggest_url = "https://duckduckgo.com/ac/?type=list",
  },
}

local configured = wayrun.web_search_engine()
local engine = ENGINES[configured]
if not engine then
  wayrun.log("unknown search engine " .. configured .. "; using google")
  engine = ENGINES.google
end

local icon = wayrun.icon(engine.icon)
local copy_icon = wayrun.icon("builtin:copy")
local summary = wayrun.t("plugin.web.summary", { engine = engine.name })

local function result_url(query)
  return engine.search_url .. wayrun.urlencode(query)
end

local function row(title, query)
  local uri = result_url(query)
  return {
    title = title,
    summary = summary,
    on_click = { type = "open", uri = uri },
    icon = icon,
    ephemeral = true,
    actions = {
      {
        title = wayrun.t("action.copy_url"),
        action = { type = "execute", command = { type = "copy", text = uri } },
        icon = copy_icon,
        id = "copy_url",
      },
    },
  }
end

return {
  {
    id = "web-search",
    name = engine.name,
    icon = icon,
    description = wayrun.t("plugin.web.ready", { engine = engine.name }),
    search = function(text)
      if text == "" then
        return {}
      end
      local body = wayrun.http.get(engine.suggest_url, { q = text }, 5000)
      local payload = wayrun.json.decode(body)
      local out = { row(wayrun.t("plugin.web.search", { query = text }), text) }
      local suggestions = type(payload) == "table" and payload[2] or nil
      if type(suggestions) == "table" then
        for _, phrase in ipairs(suggestions) do
          if type(phrase) == "string" then
            out[#out + 1] = row(phrase, phrase)
          end
        end
      end
      return out
    end,
  },
}
