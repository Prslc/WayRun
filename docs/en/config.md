# Config

`~/.config/wayrun/config.toml` holds WayRun's settings. It is generated on the
first run as a commented template and is watched, so an edit is picked up without
restarting. Every key is optional: a missing file or key keeps the built-in
default shown here.

## `[ui]`

| Key | Default | Meaning |
| --- | --- | --- |
| `locale` | *(session locale)* | Interface language: `en` or `zh_cn`. Omitted, the launcher follows `$LC_ALL`/`$LC_MESSAGES`/`$LANG`. |

Read at startup, so a change takes effect on the next launch
(`systemctl --user restart wayrun-launcher`). A value with no table of its own
falls back to English, and a blank value is the same as leaving the key out.

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
| `depth` | `3` | How many levels an unindexed search descends from each of its roots, `1` to `16`. Out-of-range values are clamped. |

Indexing happens in the background, and only once `f` or `d` is used. Set
`index = false` to search just `depth` levels under `$HOME` and leave no cache
behind; disabling both plugins does the same.
