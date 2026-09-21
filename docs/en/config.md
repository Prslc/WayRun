# Config

`~/.config/wayrun/config.toml` holds core behaviour. It is generated on the
first run as a copy of `core/default-config.toml` and is watched, so an edit is
picked up without restarting the core. Every key is optional: a missing file or
key keeps the built-in default shown here. The generated file comments every key
out, so it lists the options without pinning their current defaults; uncomment
only what you want to change.

## `[web_search]`

| Key | Default | Meaning |
| --- | --- | --- |
| `engine` | `google` | Suggestion backend: `google` or `duckduckgo`. An unknown value falls back to `google`. |

## `[font]`

| Key | Default | Meaning |
| --- | --- | --- |
| `family` | `Source Han Sans CN` | Primary shaping family, or a generic name (`serif`, `sans-serif`, `monospace`). |

The shell reads `family` at startup, so a change takes effect on the next launch
(`systemctl --user restart wayrun-launcher`). cosmic-text falls back per glyph
for anything the family lacks, so a family covering only the scripts you read
keeps the fonts you never draw out of memory. The interface size is `theme.toml`'s
`[font].size` and applies live.

## `[icon]`

| Key | Default | Meaning |
| --- | --- | --- |
| `theme` | *(desktop setting)* | Icon theme for per-row icons. Omitted, the core follows `gtk-icon-theme-name` from the GTK settings and that theme's `Inherits`. |

The icon theme is optional: the panel, badge and built-in plugin glyphs are
built into the binary, and a row icon that no theme resolves falls back to a
bundled placeholder. When set, `theme` also beats the desktop setting. Resolved
icons are cached for the core's life, so a change takes effect on the next core
start.
