# Config

`$XDG_CONFIG_HOME/wayrun/config.toml` (default
`~/.config/wayrun/config.toml`) holds WayRun's settings. The first run writes a
commented template, and the launcher watches the file, so edits apply without a
restart. Every key is optional. A missing file or key keeps the built-in
default described here.

## Edit from the command line

`wayrun config list` prints every effective setting: the defaults plus your
file. `wayrun config get <key>` prints one of them.

`wayrun config set <key> <value>` and `wayrun config unset <key>` change the
file in place. Both keep its comments and re-parse the result before writing.
Keys are dotted, such as `ui.locale` and `files.exclude`. A text value needs no
quotes unless it would parse as another TOML type:

```bash
wayrun config set files.index false
wayrun config set files.exclude '["node_modules", "target"]'
```

`locale` is read at startup, so a change needs
`systemctl --user restart wayrun-launcher`. Every other key, `font.family`
included, applies through the watchers.

## Interface language

The `[ui]` table has one key.

| Key | Default | Meaning |
| --- | --- | --- |
| `locale` | *(session locale)* | Interface language: `en` or `zh_cn`. When omitted, the launcher follows glib's `$LANGUAGE` / `$LC_ALL` / `$LC_MESSAGES` / `$LANG`. |

The launcher reads the locale once, at startup, in glib's order. It takes the
first entry of `$LANGUAGE` that has a shipped table, then `$LC_ALL`,
`$LC_MESSAGES` and finally `$LANG`. A `C` or `POSIX` base locale ignores
`$LANGUAGE`. English and Simplified Chinese ship; any other locale falls back
to English.

`[ui] locale` pins the language regardless of the session. This is what a
resident service needs, because its unit inherits the session's locale, or none
at all. Setting `Environment=LC_ALL=zh_CN.UTF-8` in the unit works too.

The choice also sets the language of text from `.desktop` files: application
names, comments and desktop action labels. A value with no table of its own
falls back to English, and a blank value acts like an absent key.

Either way the locale is read at startup, so restart the service after a
change.

## Font

The `[font]` table sets the text family.

| Key | Default | Meaning |
| --- | --- | --- |
| `family` | `sans-serif` | Primary shaping family, or a generic name (`serif`, `sans-serif`, `monospace`). |

The launcher applies this live and re-shapes with the new family when the file
changes. Glyphs the family lacks fall back to the system fonts. The interface
size lives in `theme.toml` under `[font].size` and applies live too.

## Icon theme

The `[icon]` table picks the icon theme.

| Key | Default | Meaning |
| --- | --- | --- |
| `theme` | *(desktop setting)* | Icon theme for per-row icons. When omitted, the desktop's own icon theme is used. |

The icon theme is optional. The panel and the built-in plugin glyphs are
compiled into the binary, and a row icon that no theme resolves falls back to a
bundled placeholder. `theme` beats the desktop setting.

## Files and indexing

The `[files]` table controls file and directory search.

| Key | Default | Meaning |
| --- | --- | --- |
| `index` | `true` | Let `f` and `d` find files and directories anywhere under `$HOME`, not only near its top. |
| `exclude` | `["node_modules", "target", "__pycache__"]` | Directory names the search never enters and a result never shows. A name starting with `.` (`.git`, `.cache`) is always hidden and cannot be listed back into view. The shipped names are compiled in and shown commented in `config.toml`. Writing your own list replaces them whole, and `exclude = []` searches everything but hidden names. |

### How indexing works

The index is built in the background, and only once `f` or `d` is used. Set
`index = false` to search just a few levels under `$HOME` and leave no cache
behind. Disabling both plugins has the same effect. Editing `exclude` rebuilds
the index on the next `f` or `d` search.

### Hidden and excluded names

The index never contains a hidden or excluded name, so `f` and `d` cannot find
one. Typing a path still works, as in `~/.config/niri/config.kdl` or
`.config/niri/`.
