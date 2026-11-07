# Usage

Type to search. If the first word is a keyword owned by a plugin, that plugin
answers the query. Otherwise the whole input is an app or command query. See
[Plugins](plugins.md#routing) for the routing rules.

`Enter` acts on the highlighted row. An empty query shows no rows.

## Query types

The table lists the built-in keywords. A plugin can add its own.

| Input | Action |
| --- | --- |
| `firefox` | search installed applications (name, keywords, description) |
| `f <query>` | search files by name or path |
| `d <query>` | search directories by path (multi-token) |
| `r <query>` | fuzzy-search `$PATH` executables and run one |
| `?` | show keyword modes, default functions, hints and each plugin's remembered default action |
| `lock` / `reboot` / `shutdown` | system commands |
| `2 + 3` | inline calculator |

Some features need an optional component. Copy and paste need `wl-clipboard`;
without it, the keyword returns no rows. A host registered in `plugins.toml`
adds its own keyword the same way. The full list is in
[feature dependencies](install.md#feature-dependencies).

### Path queries

A query that contains `/` or starts with `~` is a path query. If the path
exists, the launcher opens it directly and does not search by name. `~` and
relative paths resolve under `$HOME`; an absolute path such as `/etc/hosts`
opens wherever it points.

If the path does not exist yet, the launcher lists entries in the typed
directory that match the last component. A trailing `/` lists the directory
itself. Matching ignores case, even in the parent components already typed. An
existing exact path is still opened literally.

### When no provider matches

If no provider matches a plain query, the launcher lists the first level of
`$HOME` in the same way.

## Keys

| Key | Action |
| --- | --- |
| `Enter` | launch the selected result |
| `Shift+Enter` | open the selected result's action panel |
| `Alt+Enter` | in the panel, set the highlighted action as its plugin's default, or clear it |
| `↑` / `↓` | move the selection |
| `PageUp` / `PageDown` | move by a page |
| `Tab` | complete the last word or path component from the results |
| `Home` / `End` | move the caret to the start/end of the query |
| `Ctrl+A` | select the whole query |
| `Shift+←/→` | extend the selection |
| `Ctrl+C` / `Ctrl+X` | copy the selection to the clipboard |
| `Ctrl+V` | paste |
| `Delete` | delete the character after the caret |
| `Esc` | dismiss (or close the action panel) |

## Action panel

### Opening the panel

`Shift+Enter` opens a Wox-style second level for the highlighted row: the
commands that result type offers.

The panel starts with the row's own command (**Open**). Next come the actions
its plugin adds for that type. A file row adds "Open in terminal", "Reveal in
file manager" and "Copy path", then an **Open with** group for the system's
other handlers. An application row lists its `[Desktop Action …]` groups. Any
actions an external host attached come last.

A dot marks the entry `Enter` runs. **Open** carries it until a default is
remembered. A row with no command has no panel at all, and the footer hides
its actions hint.

### Groups and levels

A group carries a `›` and opens a third level instead of running. `Enter` on
**Open with** lists the handlers by application name. `Enter` runs the
highlighted one, and `Esc` returns to the row's actions. A handler can be made
the default from that level, like any other entry.

While the panel is open, `↑`/`↓` and the wheel move through it. `Enter` runs
the highlighted command or opens the highlighted group. `Esc` goes back one
level; at the top it closes the panel. `Shift+Enter` closes the panel from any
level. Typing closes the panel and returns to the field.

### Default actions

`Alt+Enter` remembers the highlighted command as the default for its plugin.
`Enter` on later rows of that plugin then runs it, and the row's own command
stays available as **Open**. The remembered action carries a dot, and the
footer shows the `Alt+Enter` hint. `Alt+Enter` on **Open**, or on the action
that is already the default, clears it, so `Enter` opens normally again. Both a
plugin's own actions and a host's actions can be made the default.

`Alt+Enter` re-sends the query and keeps the panel open on the entry you
changed, so the default dot moves to that entry.

### Footer and row indicators

While a default is in effect, the row-level footer names the action `Enter`
will run (`⏎ Open in terminal`) instead of `⏎ Launch`. The row carries the same
dot the panel puts on that action, so the state reads at a glance; a long
action name is elided. `?` lists each plugin's remembered action as
`· Enter: <action id>`.

The interface language is set in `config.toml`; see [Config](config.md#interface-language).
