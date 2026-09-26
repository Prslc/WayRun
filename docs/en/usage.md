# Usage

Type to search: the first word selects a plugin's keyword when one owns it,
otherwise the whole input is an app/command query (see
[plugins.md](plugins.md)). `Enter` acts on the highlighted row.

## Prefixes

| Input | Action |
| --- | --- |
| `firefox` | search installed applications (name, keywords, description) |
| `f <query>` | search files by name or path |
| `d <query>` | search directories by path (multi-token) |
| `r <query>` | fuzzy-search `$PATH` executables and run one |
| `w <query>` | switch focus to a matching open window |
| `c <query>` | search clipboard history (cliphist) |
| `?` | show keyword modes, default functions, hints and each plugin's remembered default action |
| `lock` / `reboot` / `shutdown` | system commands |
| `2 + 3` | inline calculator |
| _(empty)_ | show most-used items |

Some prefixes need an optional component: `w` a compositor backend, `c`
cliphist, and copy/paste `wl-clipboard`. Without it, the keyword returns no
rows. Anything else a `plugins.toml` host registers — the WayRun-Plugins
workspace's Firefox and web search among them — adds its own keyword the same
way; the full dependency list is in the
[README](../../README.md#feature-dependencies).

A query containing `/` or starting with `~` is a **path query**: the path is
opened directly when it exists, without a name search. `~` and a relative path
resolve under `$HOME`; an absolute path (`/etc/hosts`) is opened wherever it
points.

## Keys

| Key | Action |
| --- | --- |
| `Enter` | launch the selected result |
| `Shift+Enter` | open the selected result's action panel |
| `Alt+Enter` | in the panel, set the highlighted action as its plugin's default, or clear it |
| `↑` / `↓` | move the selection |
| `PageUp` / `PageDown` | move by a page |
| `Home` / `End` | move the caret to the start/end of the query |
| `Ctrl+A` | select the whole query |
| `Shift+←/→` | extend the selection |
| `Ctrl+C` / `Ctrl+X` | copy the selection to the clipboard |
| `Ctrl+V` | paste |
| `Delete` | delete the character after the caret |
| `Esc` | dismiss (or close the action panel) |

## Action panel

`Shift+Enter` opens a Wox-style second level for the highlighted row: the
commands that type of result offers. It leads with the row's own command
(**Open**), then the actions its plugin adds for that type — a file row offers
"Open in terminal", "Reveal in file manager" and "Copy path", an application row
lists its `[Desktop Action …]` groups — then anything an external host attached,
and last the launcher-level
entries: **Pin to top** / **Unpin**, plus **Remove from history** on a row the
empty-query history sourced. The dot marks the entry `Enter` runs, so **Open**
carries it until a default is remembered. A row with no command to offer has no
panel at all, so the footer hides its actions hint.

While the panel is open, `↑`/`↓` (and the wheel) move through it, `Enter` runs
the highlighted command, and `Esc` or `Shift+Enter` closes it. Typing closes the
panel and returns to the field. `Pin to top`/`Unpin` and the `Alt+Enter` gesture
re-send the query with the panel held open on the entry that changed — the pin
becomes `Unpin` in place and the default dot moves where you put it.

`Alt+Enter` remembers the highlighted command as the default for its plugin, so
`Enter` on later rows of that plugin runs it; the row's own command stays
available in the panel as **Open**. The remembered action carries a dot and the
footer shows the `Alt+Enter` hint; `Alt+Enter` on **Open** — or on the action
that is already the default — clears it, so `Enter` opens normally again.
Launcher-level actions (pin/unpin, **Remove from history**) cannot be made
default; a plugin's own actions, and a host's, can.

While a default is in effect the row-level footer names the action `Enter` will
run (`⏎ Open in terminal`) instead of `⏎ Launch`, and the row carries the same
dot the panel puts on that action, so a list of rows marked that way reads at a
glance; a long action name is elided. `?` lists each plugin's remembered action
as `· Enter: <action id>`.

## Language

The interface follows the session's locale, read once at startup: `$LC_ALL`,
then `$LC_MESSAGES`, `$LANG`, then `$LANGUAGE`'s first entry. English and
Simplified Chinese ship; any other locale falls back to English.

`config.toml`'s `[ui] locale` pins the language regardless of the session, which
is what a resident service wants: its unit inherits the session's locale, or none
at all. Setting `Environment=LC_ALL=zh_CN.UTF-8` in the unit works too. Either
way the locale is read at startup, so restart the service after a change.

## Pinned results

**Pin to top** stores the row under the exact query it was pinned on — the whole
trimmed input, keyword included — and re-emits it at the top the next time that
same string is searched. Pinning Firefox on `firefox` leads the results for
`firefox`, but not for `fire` or a bare keyword; pinning on `f report` leads
only `f report`. The empty query is its own scope, so a pin made there leads the
history. The original copy in the fresh results is dropped, so a pinned row
appears once. **Unpin** takes it back out.
