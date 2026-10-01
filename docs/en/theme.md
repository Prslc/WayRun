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

Three base roles plus the backdrop dim. A role is `#rrggbb` or `#rgb`; `dim`
takes any of those plus `#rrggbbaa`/`#rgba`, the trailing pair being its alpha.
An unparseable value is skipped and the default stays.

| Key | Default | Meaning |
| --- | --- | --- |
| `primary` | system palette | Accent: selection tint, accent bar, panel header. |
| `fg` | system palette | Text, icons and hint tints. |
| `container` | system palette | Card fill. |
| `dim` | `#0000004d` | Backdrop dim; its alpha is inline. |

`[colors]` is the shared set; `[colors.dark]` and `[colors.light]` override it
per mode (see below).

Every other surface derives from a role at a shipped alpha: the card fill is
`container` at 0.72, the selected row's tint `primary` at 0.15, the placeholder
and hints `fg` at 0.55, and so on. An unset key keeps following the system
palette, so a matugen/DMS change still recolours the rest live; setting a key
pins that one value.

A role you set stops following the system palette, while an unset role keeps
tracking it. Omitting the section is the same as a pure dynamic theme.

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
| `width` | float | `730.0` | > 0 | Card width in logical pixels; a narrower output shrinks it to fit. |
| `radius` | float | `16.0` | ≥ 0 | Card corner radius; `0` gives square corners, and the inner radii shrink with it. |
| `top_ratio` | float | `0.28` | 0–1 | Card top as a fraction of the output height. |
| `align` | string | `center` | `left`/`center`/`right` | Horizontal anchor on the output. |
| `max_rows` | int | `5` | 1–8 | Visible result rows. |

An unknown `align` keeps the default.

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
| `reduced` | bool | `false` | Same as `WAYRUN_REDUCED_MOTION=1`: both animations start at their end state. |
