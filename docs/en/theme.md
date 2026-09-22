# Theme

How WayRun is coloured, and what `~/.config/wayrun/theme.toml` controls.

There are two layers: the system palette, which comes from DankMaterialShell, and
the overrides in `theme.toml`. Both are watched, so an edit applies live.

## System palette

On a DankMaterialShell desktop the Material You palette is the one DMS generated
with matugen:

```
~/.cache/DankMaterialShell/dms-colors.json
```

It holds the light and dark palettes and the active `mode`, so switching
light/dark or changing the wallpaper is picked up live.

| WayRun | DMS, from `colors[mode]` |
| --- | --- |
| `primary` | `primary` |
| `fg` | `on_surface` |
| `container` | `surface_container_high` |

Without DMS the built-in dark palette is used, with `mode = "dark"`.

## `~/.config/wayrun/theme.toml`

Every section and every key is optional. An absent key keeps the default, an
unparseable file is ignored, and out-of-range values are clamped. The file is
written on first run as a commented template, so the options are listed without
pinning their current defaults; uncomment only what you want to change.

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

A surface defaults to its role at the shipped alpha; set the key only to change
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

A role you set stops following the system palette, while an unset role keeps
tracking it, so a matugen/DMS change still recolours the rest live. Omitting the
section is the same as a pure dynamic theme.

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

The active mode follows the system palette (`dark` when there is none), so the
tables follow a live light/dark switch with no restart. A key the active table
does not set keeps the `[colors]` value.

### `[blur]`

The card is translucent; on a compositor that offers
`ext-background-effect-v1`, the region behind it is blurred by the compositor,
which is where the frosted look comes from. A compositor without the protocol
ignores the region, so this setting then has no visible effect.

On niri the effect needs one more thing: niri auto-enables **xray** for a
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

The namespace is `WayRun`. Non-xray blur is recomputed whenever the content
underneath changes, so it costs more than the default.

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
| `radius` | float | `16.0` | ≥ 0 | Card corner radius; `0` gives square corners, and the inner radii shrink with it. |
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

The interface size, in logical pixels. One size drives every role; the rest
scale with it. The font family is not here — it lives in `config.toml`'s
`[font].family`.

| Key | Type | Default | Range | Meaning |
| --- | --- | --- | --- | --- |
| `size` | float | `14.0` | 1–96 | A result row's title; the other roles scale from it. |

### `[motion]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `entrance_ms` | int | `240` | Opacity fade when the launcher is shown. |
| `reflow_ms` | int | `150` | Card height animation. |
| `reduced` | bool | `false` | Same as `WAYRUN_REDUCED_MOTION=1`: both animations start at their end state. |
