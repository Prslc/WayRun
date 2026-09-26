# Config

`~/.config/wayrun/config.toml` holds WayRun's settings. It is generated on the
first run as a commented template and is watched, so an edit is picked up without
restarting. Every key is optional: a missing file or key keeps the built-in
default shown here.

## `[ui]`

| Key | Default | Meaning |
| --- | --- | --- |
| `locale` | *(session locale)* | Interface language: `en` or `zh_cn`. Omitted, the launcher follows `$LC_ALL`/`$LC_MESSAGES`/`$LANG`/`$LANGUAGE`. |

Read at startup, so a change takes effect on the next launch
(`systemctl --user restart wayrun-launcher`), and it also decides the language
of the text `.desktop` files contribute: application names, their comments and
desktop action labels. A value with no table of its own falls back to English,
and a blank value is the same as leaving the key out.

## `[web_search]`

| Key | Default | Meaning |
| --- | --- | --- |
| `engine` | `google` | Suggestion backend: `google` or `duckduckgo`. An unknown value falls back to `google`. |

## `[font]`

| Key | Default | Meaning |
| --- | --- | --- |
| `family` | `Source Han Sans CN` | Primary shaping family, or a generic name (`serif`, `sans-serif`, `monospace`). |

Read at startup, so a change takes effect on the next launch
(`systemctl --user restart wayrun-launcher`). Glyphs the family lacks fall back
to the system fonts. The interface size is `theme.toml`'s `[font].size` and
applies live.

## `[icon]`

| Key | Default | Meaning |
| --- | --- | --- |
| `theme` | *(desktop setting)* | Icon theme for per-row icons. Omitted, the desktop's own icon theme is used. |

The icon theme is optional: the panel, badge and built-in plugin glyphs are
built into the binary, and a row icon no theme resolves falls back to a bundled
placeholder. `theme` beats the desktop setting. A change takes effect on the next
launch.

## `[files]`

| Key | Default | Meaning |
| --- | --- | --- |
| `index` | `true` | Let `f` and `d` find files and directories anywhere under `$HOME`, not only near its top. |
| `exclude` | `["node_modules", "target", "__pycache__"]` | Directory names the search never enters and a result never shows. A name starting with `.` (`.git`, `.cache`) is always hidden and cannot be listed back into view. The shipped names are compiled in and shown commented in `config.toml`; writing your own list replaces them whole, and `exclude = []` searches everything but hidden names. |

Indexing happens in the background, and only once `f` or `d` is used. Set
`index = false` to search just a few levels under `$HOME` and leave no cache
behind; disabling both plugins does the same. Editing `exclude` makes the index
rebuild on the next `f`/`d` search. A hidden or excluded name is still reachable
by typing a path to it (`~/.config/niri/config.kdl`, `.config/niri/`).
