# Usage

The overlay is one text field. Typing searches; the first word routes to a
plugin's keyword when one owns it, otherwise the whole input is an app/command
query (see [plugins.md](plugins.md)). Results are rows, and Enter acts on the
highlighted one.

## Prefixes

| Input | Action |
| --- | --- |
| `firefox` | fuzzy-search installed applications |
| `b <query>` | search Firefox bookmarks |
| `h <query>` | search Firefox history |
| `f <query>` | search files by name or path |
| `d <query>` | search directories by path (multi-token) |
| `r <query>` | fuzzy-search `$PATH` executables and run one |
| `w <query>` | switch focus to a matching open window |
| `c <query>` | search clipboard history (cliphist) |
| `s <query>` | web search suggestions (engine set in `config.toml`) |
| `?` | show keyword modes, default functions, hints and each plugin's remembered default action |
| `lock` / `reboot` / `shutdown` | system commands |
| `2 + 3` | inline calculator |
| _(empty)_ | show most-used items |

Some prefixes need an optional component: `w` a compositor backend, `b`/`h`
Firefox, `c` cliphist, and copy/paste `wl-clipboard`. Without it, the keyword
returns no rows. The full list is in the
[README](../../README.md#feature-dependencies).

A query containing `/` or starting with `~` is a **path query**: it is stat'd
directly and opened when it exists, even past the depth-3 walk. `~` and a
relative path resolve under `$HOME`; the walk itself never leaves `$HOME`, but
an absolute path (`/etc/hosts`) is opened wherever it points.

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

`Shift+Enter` opens a Wox-style second level for the highlighted row: a list of
the commands that type of result offers. It leads with the row's own command
(**Open**), then the actions the plugin adds for that type — a file row offers
"Open in terminal", "Reveal in file manager" and "Copy path", an application row
lists its `[Desktop Action …]` groups, a bookmark or search hit offers "Copy
URL" — then anything an external host attached, and last the launcher-level
entries: **Pin to top** / **Unpin**, plus **Remove from history** on a row the
empty-query history sourced and really recorded. The dot marks the entry `Enter`
runs, so **Open** carries it until a default is remembered. A row with no command
to offer has no panel at all, so the footer hides its actions hint.

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
Launcher-level actions (pin/unpin, **Remove from history**) belong to no plugin
scope and cannot be made default; a host's own actions carry the host's scope, so
they can.

While a default is in effect the row-level footer names the action `Enter` will
run (`⏎ Open in terminal`) instead of `⏎ Launch`, and the row carries the same
dot the panel puts on that action, so a list of rows marked that way reads at a
glance; a long action name is elided. `?` lists each plugin's remembered action
as `· Enter: <action id>`.

## Language

The interface follows the session's locale, read once at startup: `$LC_ALL`,
then `$LC_MESSAGES`, then `$LANG`, then `$LANGUAGE`'s first entry, normalised to
a `locales/<locale>.yml` stem (`zh_CN.UTF-8` → `zh_cn`). English and Simplified
Chinese ship; a locale with no table of its own falls back to English, and any
Chinese variant uses the `zh_cn` table. The shell and its core child read the
same tables, so a row's action names and the footer cannot disagree.

`config.toml`'s `[ui] locale` pins the language regardless of the session, which
is what a resident service wants: its unit inherits the session's locale, or none
at all. Setting `Environment=LC_ALL=zh_CN.UTF-8` in the unit works too. Either
way the locale is read at startup, so restart the service after a change. Another
language is one more `locales/<locale>.yml` file with the same keys.

## Pinned results

**Pin to top** stores the row under the exact query it was pinned on — the whole
trimmed input, keyword included — and re-emits it at the top the next time that
same string is searched. Pinning Firefox on `firefox` leads the results for
`firefox`, but not for `fire` or a bare keyword; pinning on `b firefox` leads
only `b firefox`. The empty query is its own scope, so a pin made there leads the
history. The original copy in the fresh results is dropped, so a pinned row
appears once. **Unpin** takes it back out; the pins live in `usage.db`.
