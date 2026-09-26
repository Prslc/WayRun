-- Firefox bookmarks and history from places.sqlite, as one host with two
-- plugins: both read the same newest-profile snapshot.

local bookmarks_icon = wayrun.icon("builtin:bookmark")
local history_icon = wayrun.icon("builtin:clock")
local copy_icon = wayrun.icon("builtin:copy")
local copy_title = wayrun.t("action.copy_url")

local function copy_action(uri)
  if uri:sub(1, 4) ~= "http" then
    return nil
  end
  return {
    title = copy_title,
    action = { type = "execute", command = { type = "copy", text = uri } },
    icon = copy_icon,
    id = "copy_url",
  }
end

local function newest_db()
  local home = wayrun.home()
  if not home then
    return nil
  end
  local best, best_mtime
  local bases = { home .. "/.mozilla/firefox", home .. "/.config/mozilla/firefox" }
  for _, base in ipairs(bases) do
    for _, entry in ipairs(wayrun.fs.list(base) or {}) do
      local db = base .. "/" .. entry .. "/places.sqlite"
      local stat = wayrun.fs.stat(db)
      if stat and (not best_mtime or stat.mtime_ns > best_mtime) then
        best, best_mtime = db, stat.mtime_ns
      end
    end
  end
  return best
end

-- One connection per profile snapshot; the snapshot's path encodes
-- (mtime, size), so a changed file opens a new one and an unchanged reuses.
local snapshot
local function handle()
  local db = newest_db()
  if not db then
    return nil
  end
  local stat = wayrun.fs.stat(db)
  if snapshot and snapshot.db == db and snapshot.mtime_ns == stat.mtime_ns and snapshot.size == stat.size then
    return snapshot.handle
  end
  local opened = wayrun.sqlite.snapshot(db)
  snapshot = { db = db, mtime_ns = stat.mtime_ns, size = stat.size, handle = opened }
  return opened
end

local QUERIES = {
  bookmarks = [[
    SELECT COALESCE(NULLIF(moz_bookmarks.title, ''), moz_places.title) AS title,
           moz_places.url
    FROM moz_bookmarks
    JOIN moz_places ON moz_bookmarks.fk = moz_places.id
    WHERE moz_places.url <> ''
      AND (?1 = '' OR COALESCE(NULLIF(moz_bookmarks.title, ''), moz_places.title) LIKE ?2 OR moz_places.url LIKE ?2)
    ORDER BY moz_bookmarks.dateAdded DESC
    LIMIT 50
  ]],
  history = [[
    SELECT moz_places.title, moz_places.url
    FROM moz_places
    WHERE moz_places.url <> ''
      AND moz_places.last_visit_date IS NOT NULL
      AND (?1 = '' OR moz_places.title LIKE ?2 OR moz_places.url LIKE ?2)
    ORDER BY moz_places.last_visit_date DESC
    LIMIT 50
  ]],
}

local function rows(mode, text, icon)
  local open = handle()
  if not open then
    return {}
  end
  local records = wayrun.sqlite.query(open, QUERIES[mode], { text, "%" .. text .. "%" })
  local out = {}
  for _, record in ipairs(records) do
    local item = {
      title = record.title or "[no title]",
      summary = record.url,
      on_click = { type = "open", uri = record.url },
      icon = icon,
      ephemeral = false,
    }
    local copy = copy_action(record.url)
    if copy then
      item.actions = { copy }
    end
    out[#out + 1] = item
  end
  return out
end

local function searcher(mode, icon)
  return function(text)
    if text == "" then
      return {}
    end
    return rows(mode, text, icon)
  end
end

return {
  {
    id = "firefox-bookmarks",
    name = wayrun.t("plugin.bookmarks.name"),
    icon = bookmarks_icon,
    description = wayrun.t("plugin.bookmarks.ready"),
    search = searcher("bookmarks", bookmarks_icon),
  },
  {
    id = "firefox-history",
    name = wayrun.t("plugin.history.name"),
    icon = history_icon,
    description = wayrun.t("plugin.history.ready"),
    search = searcher("history", history_icon),
  },
}
