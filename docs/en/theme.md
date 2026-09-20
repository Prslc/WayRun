# Theme

How WayRun is coloured and what `~/.config/wayrun/theme.toml` controls.

Two layers:

1. **System palette** — the core reads DMS's `dms-colors.json` and sends the role
   colours and the active `mode` to the shell over the IPC stream.
2. **`theme.toml`** — the shell reads it and applies its overrides to those roles.

The two files are watched by the core and the shell respectively; either edit
applies live, nothing restarts.

## System palette

On a DankMaterialShell desktop the core reads the Material You palette DMS
generated with matugen:

```
~/.cache/DankMaterialShell/dms-colors.json
```

The file holds both palettes at once plus the active `mode`, so switching
light/dark or changing the wallpaper is a plain file edit and is picked up live.

Roles are mapped like this:

| WayRun | DMS, from `colors[mode]` |
| --- | --- |
| `primary` | `primary` |
| `on_primary` | `on_primary` |
| `bg` | `background` |
| `fg` | `on_surface` |
| `container` | `surface_container_high` |

The shell currently draws with `primary`, `fg` and `container`; `bg` and
`on_primary` are carried but unused. The core also sends the active `mode`, which
the shell uses to pick `theme.toml`'s per-mode colour table.

Without DMS the built-in dark palette is used, with `mode = "dark"`. Support for
the freedesktop appearance portal (`org.freedesktop.appearance`: `color-scheme`
and `accent-color`), which is what GTK/libadwaita reads on GNOME and KDE, is
planned.

## `~/.config/wayrun/theme.toml`

Every section and every key is optional. An absent key keeps the default, an
unparseable file is ignored, and out-of-range values are clamped. The file is
watched, so an edit applies without restarting.

The first shell run writes `~/.config/wayrun/theme.toml` as a commented template.
Every key in it is commented out, so it lists the options inline without pinning
their current defaults; uncomment only what you want to change.

### `[colors]`

Three base roles, then the surfaces they derive. A base role is `#rrggbb` or
`#rgb`; a surface is `#rrggbb`, `#rgb`, `#rrggbbaa` or `#rgba`, the trailing
pair being its alpha. An unparseable value is skipped and the default stays.

| Key | Default | Meaning |
| --- | --- | --- |
| `primary` | system palette | Accent: selection tint, accent bar, panel header. |
| `fg` | system palette | Text, icons, field and hint tints. |
| `container` | system palette | Card fill. |
| `follow_system` | `false` | Ignore the base roles **and** every surface below; track the system palette entirely. |

`[colors]` is the shared set; `[colors.dark]` and `[colors.light]` override it
per mode (see below).

A surface's default is its role at the shipped alpha; set the key only to change
that surface on purpose, alpha included.

| Key | Derives from | Default (alpha) |
| --- | --- | --- |
| `card` | `container` | `#24283bb8` (0.72) card fill |
| `field` | `fg` | `#c0caf514` (0.08) search field |
| `selection` | `primary` | `#7aa2f726` (0.15) selected row tint |
| `hover` | `primary` | `#7aa2f714` (0.08) hovered row tint |
| `hairline` | white | `#ffffff59` (0.35) 1px card border |
| `muted` | `fg` | `#c0caf58c` (0.55) placeholder, magnifier, ✕ and hints |
| `summary` | `fg` | `#c0caf5b3` (0.70) row summary |
| `footer` | `fg` | `#c0caf580` (0.50) footer hint text |
| `accent` | `primary` | `#7aa2f7` (1.0) selected row accent bar |
| `dim` | — | `#0000004d` (0.30) backdrop dim |

The overlay is per field: a role you set stops following the system palette,
while an unset role keeps tracking it, so a matugen/DMS change still recolours
the rest live. Omitting the section is the same as a pure dynamic theme.

#### Per-mode overrides

A hand-picked colour is static, but the system palette switches between light
and dark. `[colors.dark]` and `[colors.light]` accept the same keys as
`[colors]` and layer on top of it while that mode is active, so a colour that
only works in one mode can differ while everything else stays shared:

```toml
[colors]
primary = "#7aa2f7"          # shared by both modes

[colors.dark]
fg = "#c0caf5"

[colors.light]
fg = "#1f2430"
```

The active mode is the one the core resolved (the DMS `mode`, and `dark` when the
system palette is unavailable), so the tables follow a live light/dark switch with
no restart. A key the active table does not set keeps the `[colors]` value.

### `[blur]`

The shell never blurs pixels itself. It draws a translucent card, and when a
compositor offers `ext-background-effect-v1` it hands that compositor the
card's rounded rectangle as a region to blur behind. The compositor does the
work; that is the frosted look. A compositor without the protocol ignores the
region, so this setting then has no visible effect.

On niri the effect needs one more thing. niri auto-enables **xray** for a
client's `ext-background-effect` request, which blurs the wallpaper and ignores
the windows below. For the card to blur the content actually behind it, add a
layer rule that turns xray off:

```kdl
layer-rule {
    match namespace="WayRun"
    background-effect {
        xray false
    }
}
```

The namespace is `WayRun`. niri recomputes non-xray blur whenever the content
underneath changes, so it is costlier than the default.

Hyprland does not implement `ext-background-effect-v1`, so it ignores the
region; turn on its own blur and blur the layer by namespace instead:

```ini
decoration {
    blur {
        enabled = true
    }
}
layerrule = blur, WayRun
```

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | Declare the blur region. `false` declares nothing, so the backdrop stays sharp; the card's translucent fill is unchanged. |

### `[layout]`

| Key | Type | Default | Range | Meaning |
| --- | --- | --- | --- | --- |
| `radius` | float | `16.0` | ≥ 0 | Card corner radius. `0` is square corners; the inner radii follow it down so nothing pokes outside. |
| `field_radius` | float | derived (9.0) | ≥ 0 | Field corner radius; defaults to the `radius`-derived 9 and is still capped by the card. |
| `row_radius` | float | derived (8.0) | ≥ 0 | Row tint radius. |
| `chip_radius` | float | derived (6.0) | ≥ 0 | Keyword chip radius. |
| `hairline_width` | float | `1.0` | 0–8 | Card border stroke width. |
| `accent_width` | float | `3.0` | 0–40 | Selected row's accent bar width. |
| `accent_height` | float | `28.0` | 0–200 | Selected row's accent bar height. |
| `width_ratio` | float | `0.38` | > 0 | Card width as a fraction of the output width. |
| `width_min` | float | `560.0` | > 0 | Lower clamp for the width. |
| `width_max` | float | `760.0` | > 0 | Upper clamp for the width. |
| `top_ratio` | float | `0.28` | 0–1 | Card top as a fraction of the output height. |
| `align` | string | `center` | `left`/`center`/`right` | Horizontal anchor on the output. |
| `offset_x` | float | `0.0` | any | Nudge after the width and anchor are resolved. |
| `offset_y` | float | `0.0` | any | Nudge after `top_ratio` is resolved. |
| `max_rows` | int | `5` | 1–8 | Visible result rows. |

If `width_min` ends up greater than `width_max`, it is pulled down to
`width_max`. An unknown `align` keeps the default.

### `[font]`

The interface size, in logical pixels. Every role keeps its shipped ratio to it
(query 1.286×, title 1×, summary 0.857×, suggestion 0.786×, icon 2.143×, badge
1.071×). The shaping family is **not** here; it lives in `config.toml`'s
`[font].family` because the core owns text shaping.

| Key | Type | Default | Range | Meaning |
| --- | --- | --- | --- | --- |
| `size` | float | `14.0` | 1–96 | A result row's title; the other roles scale from it. |

### `[motion]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `entrance_ms` | int | `240` | Opacity fade when the launcher is shown. |
| `reflow_ms` | int | `150` | Card height animation. |
| `reduced` | bool | `false` | Same as `WAYRUN_REDUCED_MOTION=1`; either one starts both animations at their end state and asks for no frames. |

## Example

```toml
# ~/.config/wayrun/theme.toml — every key is optional
[colors]
primary = "#7aa2f7"
fg = "#c0caf5"
container = "#24283b"
follow_system = false

# a surface is the base role at the shipped alpha unless set here
card = "#24283bcc"
muted = "#c0caf5a0"

[blur]
enabled = true

[layout]
radius = 16.0
field_radius = 9.0
row_radius = 8.0
chip_radius = 6.0
hairline_width = 1.0
accent_width = 3.0
accent_height = 28.0
width_ratio = 0.38
width_min = 560.0
width_max = 760.0
top_ratio = 0.28
align = "center"
offset_x = 0.0
offset_y = 0.0
max_rows = 5

[font]
size = 14.0

[motion]
entrance_ms = 240
reflow_ms = 150
reduced = false
```
