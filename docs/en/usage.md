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
| `?` | show keyword modes, default functions, and hints |
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
the commands that type of result offers. Its entries come from the plugin that
produced the row, so a file row offers "Reveal in file manager", an application
row lists its `[Desktop Action …]` groups, a bookmark or search hit offers "Copy
URL", and an external host may attach its own. Every actionable row also gets
the launcher-level **Pin to top** / **Unpin**; a row in the empty-query history
— and only one that was really recorded there — additionally offers **Remove
from history**. A row with no command to offer has no panel at all, so the
footer hides its actions hint.

While the panel is open, `↑`/`↓` (and the wheel) move through it, `Enter` runs
the highlighted command, and `Esc` or `Shift+Enter` closes it. Typing closes the
panel and returns to the field.

`Alt+Enter` remembers the highlighted command as the default for its plugin, so
`Enter` on later rows of that plugin runs it; the row's own command stays
available in the panel as **Open**. The remembered action carries a dot and the
footer shows the `Alt+Enter` hint; repeat the gesture to clear it. Launcher-level
and host actions have no stable id and cannot be made default.

## Pinned results

**Pin to top** stores the row under the exact query it was pinned on — the whole
trimmed input, keyword included — and re-emits it at the top the next time that
same string is searched. Pinning Firefox on `firefox` leads the results for
`firefox`, but not for `fire` or a bare keyword; pinning on `b firefox` leads
only `b firefox`. The empty query is its own scope, so a pin made there leads the
history. The original copy in the fresh results is dropped, so a pinned row
appears once. **Unpin** takes it back out; the pins live in `usage.db`.
