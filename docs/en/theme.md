# Theme

WayRun has two colour layers: the system palette from the desktop, and the
overrides in `$XDG_CONFIG_HOME/wayrun/theme.toml` (default
`~/.config/wayrun/theme.toml`). The launcher watches both, so an edit applies
live.

## System palette

The palette follows the session.

### KDE Plasma

On KDE Plasma the palette is the active colour scheme in
`~/.config/kdeglobals`. KDE merges the applied scheme's roles into that file,
and writes an explicit or wallpaper-derived accent back to
`[General] AccentColor`.

There is no mode key. The luminance of the window background decides the mode,
so a light/dark switch is picked up live.

| WayRun | KDE, from `kdeglobals` |
| --- | --- |
| `mode` | luminance of `[Colors:Window] BackgroundNormal` |
| `primary` | `[General] AccentColor`, else `[Colors:Window] DecorationFocus` |
| `fg` | `[Colors:Window] ForegroundNormal` |
| `container` | `[Colors:Button] BackgroundNormal` |
| `bg` | `[Colors:Window] BackgroundNormal` |

### DankMaterialShell

On a DankMaterialShell desktop the palette is the Material You palette DMS
generates with matugen, in `~/.cache/DankMaterialShell/dms-colors.json`. The
file holds the light and dark palettes and the active mode. Switching
light/dark, or changing the wallpaper, is picked up live.

| WayRun | DMS, from `colors[mode]` |
| --- | --- |
| `primary` | `primary` |
| `fg` | `on_surface` |
| `container` | `surface_container_high` |

### Precedence and fallback

On a KDE session the launcher reads `kdeglobals`, even if a DMS file is left
over from another desktop. With neither source it uses the built-in dark
palette, with `mode = "dark"`.

## Theme overrides

Every section and key is optional. An absent key keeps its default, an
unparseable file is ignored, and out-of-range values are clamped. The first run
writes a commented template, so the options are listed without pinning their
current defaults. Uncomment only what you want to change.

### Colors

The `[colors]` table sets three base roles plus the backdrop dim. A role is
`#rrggbb` or `#rgb`. `dim` also accepts `#rrggbbaa` and `#rgba`, where the
trailing pair is its alpha. An unparseable value is skipped and the default
stays.

| Key | Default | Meaning |
| --- | --- | --- |
| `primary` | system palette | Accent: selection tint, accent bar, panel header. |
| `fg` | system palette | Text, icons and hint tints. |
| `container` | system palette | Card fill. |
| `dim` | `#0000004d` | Backdrop dim; its alpha is inline. |

`[colors]` is the shared set. `[colors.dark]` and `[colors.light]` override it
per mode; see below.

Every other surface derives from a role at a fixed alpha. For example, the card
fill is `container` at 0.72 (0.85 in dark mode), and the selected row's tint is
`primary` at 0.15. The `[surfaces]` table lists the full set.

An unset key keeps following the system palette, so a matugen or DMS change
still recolours it. Setting a key pins that value. A role you set stops
following the palette; an unset role keeps tracking it. Omitting the section is
the same as a pure dynamic theme.

#### Per-mode overrides

A hand-picked colour is static, but the system palette switches between light
and dark. `[colors.dark]` and `[colors.light]` accept the same keys as
`[colors]` and layer on top of it while that mode is active. A colour that only
works in one mode can therefore differ while everything else stays shared:

```toml
[colors]
primary = "#7aa2f7"          # shared by both modes

[colors.dark]
fg = "#c0caf5"

[colors.light]
fg = "#1f2430"
```

The active mode follows the system palette (`dark` when there is none), so the
tables follow a live switch with no restart. A key the active table does not set
keeps the `[colors]` value.

### Surfaces

The `[surfaces]` table pins individual surfaces the renderer derives from the
base roles. An entry sets `color`, `alpha`, or both. An omitted field keeps
deriving, so a `[colors]` change or a live palette change still reaches it.

```toml
[surfaces]
card = { alpha = 0.90 }                 # keep following container, more opaque
selection = { color = "#7aa2f7", alpha = 0.20 }
hairline = { color = "#ffffff" }        # keep the mode's shipped alpha
```

`color` is `#rrggbb` or `#rgb`; `alpha` is 0–1 and is clamped. An unparseable
colour is ignored. The `hairline` entry also takes `width` in logical pixels
(0–16, default 1.0; `0` hides the edge), which layers like `color` and `alpha`.
`dim` is here as well: it layers over `[colors].dim`, which keeps the
plain-string spelling.

| Key | Derived from | Shipped alpha |
| --- | --- | --- |
| `card` | `container` | 0.72 (0.85 dark) |
| `selection` | `primary` | 0.15 |
| `hover` | `primary` | 0.08 |
| `preview` | `primary` | 0.05 |
| `hairline` | black in light, white in dark | 0.10 (0.12 dark) |
| `muted` | `fg` | 0.55 |
| `summary` | `fg` | 0.70 |
| `footer` | `fg` | 0.50 |
| `accent` | `primary` | 1.00 |
| `dim` | `#000000` | 0.30 |

`[surfaces]` is the shared table. `[surfaces.dark]` and `[surfaces.light]` take
the same keys and layer on it surface by surface and field by field while that
mode is active. A mode can change only the colour, only the alpha, or only the
width:

```toml
[surfaces]
hairline = { alpha = 0.12 }

[surfaces.dark]
hairline = { color = "#ffffff", width = 1.5 }

[surfaces.light]
hairline = { color = "#000000" }
```

### Blur

The `[blur]` table declares the blur region. The card is translucent. On a
compositor that offers `ext-background-effect-v1`, the compositor blurs the
region behind it, which gives the frosted look. A compositor without the
protocol ignores the region, so the setting has no visible effect there.

#### niri

On niri the effect needs one more step. niri auto-enables **xray** for a
client's `ext-background-effect` request; xray blurs the wallpaper and ignores
the windows below. To blur the content actually behind the card, add a layer
rule that turns xray off:

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

#### Hyprland

Hyprland does not implement `ext-background-effect-v1`, so it ignores the
region. Turn on its own blur and blur the layer by namespace:

```ini
decoration {
    blur {
        enabled = true
    }
}
layerrule = blur, WayRun
```

#### KWin

KWin implements `ext-background-effect-v1`, so Plasma blurs the card once the
Blur desktop effect is enabled. No layer rule is needed. The launcher maps to
the `utility` namespace, which KWin's open and close effects skip.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | Declare the blur region. `false` declares nothing, so the backdrop stays sharp; the card's translucent fill is unchanged. |

### Preview

The `[preview]` table has one key.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | Show the split preview column for the selected row. `false` keeps the list full width; plugin previews are ignored. |

### Footer

The `[footer]` table has one key.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | Show the hint bar under the results: keys, result count and search status. `false` hides it and gives its band back to the card. |

### Layout

The `[layout]` table sets the card geometry.

| Key | Type | Default | Range | Meaning |
| --- | --- | --- | --- | --- |
| `width` | float | `730.0` | > 0 | Card width in logical pixels; a narrower output shrinks it to fit. |
| `radius` | float | `16.0` | ≥ 0 | Card corner radius; `0` gives square corners, and the inner radii shrink with it. |
| `top_ratio` | float | `0.20` | 0–1 | Card top as a fraction of the output height. |
| `align` | string | `center` | `left`/`center`/`right` | Horizontal anchor on the output. |
| `max_rows` | int | `5` | 1–8 | Visible result rows. |
| `panel_header` | float | `28.0` | ≥ 0 | Action panel header band above the actions. |

An unknown `align` keeps the default.

### Font

The `[font]` table sets the text family and the interface size in logical
pixels. One size drives every role; the rest scale with it. Glyphs the family
lacks fall back to the system fonts.

| Key | Type | Default | Range | Meaning |
| --- | --- | --- | --- | --- |
| `family` | string | `sans-serif` | — | Primary shaping family, or a generic name (`serif`, `sans-serif`, `monospace`). |
| `size` | float | `14.0` | 1–96 | A result row's title; the other roles scale from it. |

Both apply live; a family change re-shapes every cached line.

### Motion

The `[motion]` table has these keys.

| Key | Type | Default | Range | Meaning |
| --- | --- | --- | --- | --- |
| `reduced` | bool | `false` | — | Same as `WAYRUN_REDUCED_MOTION=1`: turns animations off. The entrance, the card height spring and result-row motion start at their end state, and no animation frames are requested. The caret still blinks. This wins over `entrance_ms`. |
| `entrance_ms` | integer | `240` | 0–2000 | How long the entrance fade and rise take, in milliseconds. `0` skips the entrance; the card height spring and result-row motion still run. `reduced = true` still wins. |

Both apply live. Lowering `entrance_ms` mid-fade settles it at once; raising it
lets the current fade finish from when the show began.
