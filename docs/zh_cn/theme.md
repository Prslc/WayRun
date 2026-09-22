# 主题

WayRun 的配色方式，以及 `~/.config/wayrun/theme.toml` 控制的内容。

分两层：系统调色板（来自 DankMaterialShell）与 `theme.toml` 里的覆盖。两者都被监听，
改动实时生效。

## 系统调色板

在 DankMaterialShell 桌面上，Material You 调色板来自 DMS 用 matugen 生成的文件：

```
~/.cache/DankMaterialShell/dms-colors.json
```

该文件同时保存亮/暗两套调色板与当前 `mode`，因此切换亮暗或更换壁纸都会被实时捕获。

| WayRun | DMS，取自 `colors[mode]` |
| --- | --- |
| `primary` | `primary` |
| `fg` | `on_surface` |
| `container` | `surface_container_high` |

没有 DMS 时使用内置深色调色板（`mode = "dark"`）。

## `~/.config/wayrun/theme.toml`

所有 section 与键均为可选。缺省键保留默认值，无法解析的文件被忽略，越界值会被 clamp。
首次运行时会写入一份带注释的模板，因此它把可选项列在字段旁，却不会把当前默认值钉死在文件里；
只取消注释你想改的键即可。

### `[colors]`

先三个基础角色，再由它们派生出各个表面（surface）。基础角色为 `#rrggbb` 或 `#rgb`；
表面为 `#rrggbb`、`#rgb`、`#rrggbbaa` 或 `#rgba`，末尾一对是它的 alpha。无法解析的值会被跳过并保留默认。

| 键 | 默认值 | 含义 |
| --- | --- | --- |
| `primary` | 系统调色板 | 强调色：选中底色、强调条、面板标题。 |
| `fg` | 系统调色板 | 文字、图标、搜索框与提示的底色。 |
| `container` | 系统调色板 | 卡片填充。 |
| `follow_system` | `false` | 忽略基础角色**及**下面全部表面，完全跟随系统调色板。 |

`[colors]` 是共享层；`[colors.dark]` 与 `[colors.light]` 按模式在其上覆盖（见下）。

表面的默认值是“对应角色 + 既定 alpha”；只有确实想改某个表面时才写该键，alpha 一并写在颜色里。

| 键 | 派生自 | 默认值（alpha） |
| --- | --- | --- |
| `card` | `container` | `#24283bb8`（0.72）卡片填充 |
| `field` | `fg` | `#c0caf514`（0.08）搜索框 |
| `selection` | `primary` | `#7aa2f726`（0.15）选中行底色 |
| `hover` | `primary` | `#7aa2f714`（0.08）悬停行底色 |
| `hairline` | 白色 | `#ffffff59`（0.35）1px 卡片描边 |
| `muted` | `fg` | `#c0caf58c`（0.55）占位符、放大镜、✕ 与提示 |
| `summary` | `fg` | `#c0caf5b3`（0.70）结果行摘要 |
| `footer` | `fg` | `#c0caf580`（0.50）底部提示文字 |
| `accent` | `primary` | `#7aa2f7`（1.0）选中行强调条 |
| `dim` | — | `#0000004d`（0.30）背景压暗 |

覆盖是逐字段的：设了哪个角色，该角色就不再跟随系统调色板；没设的角色继续实时跟随，
因此 matugen / DMS 换色时其余部分仍会更新。整段不写等同于纯动态取色。

#### 按模式覆盖

手调的颜色是静态的，而系统调色板会在亮/暗之间切换。`[colors.dark]` 与 `[colors.light]`
接受与 `[colors]` 相同的键，在该模式生效时叠加其上，因此只在一种模式下成立的颜色可以单独设置，
其余保持共享：

```toml
[colors]
primary = "#7aa2f7"          # 两种模式共享

[colors.dark]
fg = "#c0caf5"

[colors.light]
fg = "#1f2430"
```

生效模式跟随系统调色板（没有系统调色板时为 `dark`），因此切换亮暗会实时跟随，无需重启。
当前模式的表里没写的键，仍取 `[colors]` 的值。

### `[blur]`

卡片是半透明的；合成器支持 `ext-background-effect-v1` 时，卡片背后的区域由合成器模糊，
磨砂观感即来源于此。合成器若没有该协议，会忽略该区域，此设置也就没有可见效果。

在 niri 上还需要额外一条规则。对于客户端通过 `ext-background-effect` 发起的请求，niri 会**默认自动开启 xray**，
即只模糊壁纸、忽略下方窗口。若要让卡片模糊其真正背后的内容，需要加一条关闭 xray 的 layer rule：

```kdl
layer-rule {
    match namespace="WayRun"
    background-effect {
        xray false
    }
}
```

其中 namespace 为 `WayRun`。非 xray 模糊会在其下方内容变化时重新计算，因此开销高于默认。

Hyprland 不实现 `ext-background-effect-v1`，会忽略该区域；改为打开它自己的模糊，并按 namespace 模糊该图层：

```ini
decoration {
    blur {
        enabled = true
    }
}
layerrule = blur, WayRun
```

| 键 | 类型 | 默认值 | 含义 |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | 是否向合成器声明背景模糊区域。`false` 时不再声明，背景保持清晰；卡片的半透明填充不变。 |

### `[layout]`

| 键 | 类型 | 默认值 | 范围 | 含义 |
| --- | --- | --- | --- | --- |
| `radius` | float | `16.0` | ≥ 0 | 卡片圆角；`0` 即直角，内层圆角随之收缩。 |
| `field_radius` | float | 派生（9.0） | ≥ 0 | 搜索框圆角；默认由 `radius` 派生，仍受卡片圆角限制。 |
| `row_radius` | float | 派生（8.0） | ≥ 0 | 结果行底色圆角。 |
| `chip_radius` | float | 派生（6.0） | ≥ 0 | 关键词 chip 圆角。 |
| `hairline_width` | float | `1.0` | 0–8 | 卡片描边宽度。 |
| `accent_width` | float | `3.0` | 0–40 | 选中行强调条宽度。 |
| `accent_height` | float | `28.0` | 0–200 | 选中行强调条高度。 |
| `width_ratio` | float | `0.38` | > 0 | 卡片宽度占输出宽度的比例。 |
| `width_min` | float | `560.0` | > 0 | 宽度下限。 |
| `width_max` | float | `760.0` | > 0 | 宽度上限。 |
| `top_ratio` | float | `0.28` | 0–1 | 卡片顶部占输出高度的比例。 |
| `align` | string | `center` | `left`/`center`/`right` | 在输出上的水平锚点。 |
| `offset_x` | float | `0.0` | 任意 | 宽度与锚点算定后的水平微调。 |
| `offset_y` | float | `0.0` | 任意 | `top_ratio` 算定后的垂直微调。 |
| `max_rows` | int | `5` | 1–8 | 可见结果行数。 |

若解析后 `width_min` 大于 `width_max`，会把 `width_min` 拉到 `width_max`。`align` 无法识别时保留默认值。

### `[font]`

界面尺寸，逻辑像素。一个尺寸驱动全部角色，其余角色随之缩放。字体族不在这里——它位于
`config.toml` 的 `[font].family`。

| 键 | 类型 | 默认值 | 范围 | 含义 |
| --- | --- | --- | --- | --- |
| `size` | float | `14.0` | 1–96 | 结果行标题；其余角色由它缩放。 |

### `[motion]`

| 键 | 类型 | 默认值 | 含义 |
| --- | --- | --- | --- |
| `entrance_ms` | int | `240` | 显示时的透明度淡入时长。 |
| `reflow_ms` | int | `150` | 卡片高度动画时长。 |
| `reduced` | bool | `false` | 等价于 `WAYRUN_REDUCED_MOTION=1`：两段动画都直接停在终态。 |
