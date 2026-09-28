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

三个基础角色，加一个背景压暗 `dim`。角色为 `#rrggbb` 或 `#rgb`；`dim` 另接受
`#rrggbbaa` / `#rgba`，末尾一对是它的 alpha。无法解析的值会被跳过并保留默认。

| 键 | 默认值 | 含义 |
| --- | --- | --- |
| `primary` | 系统调色板 | 强调色：选中底色、强调条、面板标题。 |
| `fg` | 系统调色板 | 文字、图标与提示的底色。 |
| `container` | 系统调色板 | 卡片填充。 |
| `dim` | `#0000004d` | 背景压暗；alpha 内联写在颜色里。 |

`[colors]` 是共享层；`[colors.dark]` 与 `[colors.light]` 按模式在其上覆盖（见下）。

其余表面全部由角色 + 既定 alpha 派生：卡面是 `container`@0.72，选中行底色是 `primary`@0.15，
占位符与提示是 `fg`@0.55，等等。没设的键继续实时跟随系统调色板，因此 matugen / DMS
换色时其余部分仍会更新；设了的键则钉住该值。整段不写等同于纯动态取色。

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
| `width` | float | `730.0` | > 0 | 卡片宽度（逻辑像素）；输出更窄时按屏宽收缩。 |
| `radius` | float | `16.0` | ≥ 0 | 卡片圆角；`0` 即直角，内层圆角随之收缩。 |
| `top_ratio` | float | `0.28` | 0–1 | 卡片顶部占输出高度的比例。 |
| `align` | string | `center` | `left`/`center`/`right` | 在输出上的水平锚点。 |
| `max_rows` | int | `5` | 1–8 | 可见结果行数。 |

`align` 无法识别时保留默认值。

### `[font]`

界面尺寸，逻辑像素。一个尺寸驱动全部角色，其余角色随之缩放。字体族不在这里——它位于
`config.toml` 的 `[font].family`。

| 键 | 类型 | 默认值 | 范围 | 含义 |
| --- | --- | --- | --- | --- |
| `size` | float | `14.0` | 1–96 | 结果行标题；其余角色由它缩放。 |

### `[motion]`

| 键 | 类型 | 默认值 | 含义 |
| --- | --- | --- | --- |
| `reduced` | bool | `false` | 等价于 `WAYRUN_REDUCED_MOTION=1`：两段动画都直接停在终态。 |
