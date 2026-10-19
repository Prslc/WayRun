# 主题

WayRun 的配色方式，以及 `$XDG_CONFIG_HOME/wayrun/theme.toml`（默认
`~/.config/wayrun/theme.toml`）控制的内容。分两层：系统调色板（来自
DankMaterialShell）与 `theme.toml` 的覆盖。两者都会被监听，改动实时生效。

## 系统调色板

在 DankMaterialShell 桌面上，Material You 调色板来自 DMS 用 matugen 生成的文件：

```
~/.cache/DankMaterialShell/dms-colors.json
```

该文件同时保存亮色与暗色两套调色板以及当前模式，因此切换亮暗或更换壁纸都会被实时
捕获。

| WayRun | DMS，取自 `colors[mode]` |
| --- | --- |
| `primary` | `primary` |
| `fg` | `on_surface` |
| `container` | `surface_container_high` |

没有 DMS 时使用内置深色调色板（`mode = "dark"`）。

## `$XDG_CONFIG_HOME/wayrun/theme.toml`

所有 section 与键均为可选。缺省的键保留默认值；无法解析的文件会被忽略；超出范围的
值会被限制在范围内。首次运行会写入一份带注释的模板：可选项列在字段旁，默认值不写死；
取消注释想改的键即可。

### `[colors]`

三个基础角色，加一个背景压暗 `dim`。角色为 `#rrggbb` 或 `#rgb`；`dim` 还接受
`#rrggbbaa` / `#rgba`，最后一对是透明度。无法解析的值会被跳过，保留默认。

| 键 | 默认值 | 含义 |
| --- | --- | --- |
| `primary` | 系统调色板 | 强调色：选中底色、强调条、面板标题。 |
| `fg` | 系统调色板 | 文字、图标与提示的底色。 |
| `container` | 系统调色板 | 卡片填充。 |
| `dim` | `#0000004d` | 背景压暗；透明度内联写在颜色里。 |

`[colors]` 是共享层；`[colors.dark]` 与 `[colors.light]` 按模式在其上覆盖（见下）。

其余表面全部由角色加固定透明度派生：卡面是 `container`@0.72（暗色 0.85），描边在
浅色下是黑色@0.10（暗色下白色@0.12），选中行底色是 `primary`@0.15，占位符与提示是
`fg`@0.55，等等。没设的键继续实时跟随系统调色板，因此 matugen / DMS 换色时其余部分
仍会更新；设了的键则固定该值。每个派生表面也可以在下面的 `[surfaces]` 里单独固定。
整段不写等同于纯动态取色。

#### 按模式覆盖

手调的颜色是静态的，而系统调色板会在亮暗之间切换。`[colors.dark]` 与
`[colors.light]` 接受与 `[colors]` 相同的键，在该模式下叠加在共享层之上；因此只在
一种模式下成立的颜色可以单独设置，其余保持共享：

```toml
[colors]
primary = "#7aa2f7"          # 两种模式共享

[colors.dark]
fg = "#c0caf5"

[colors.light]
fg = "#1f2430"
```

生效模式跟随系统调色板（没有系统调色板时为 `dark`），切换亮暗会实时跟随，无需重启。
当前模式的表里没写的键，仍取 `[colors]` 的值。

### `[surfaces]`

渲染器绘制的各个表面由基础角色派生，也可以逐个固定。每项可写 `color`、`alpha` 或
两者；没写的字段继续派生，因此 `[colors]` 的改动或调色板的实时变化仍会作用到它。

```toml
[surfaces]
card = { alpha = 0.90 }                 # 颜色继续跟随 container，只把卡片调得更不透明
selection = { color = "#7aa2f7", alpha = 0.20 }
hairline = { color = "#ffffff" }        # 透明度保持该模式下的默认值
```

`color` 为 `#rrggbb` 或 `#rgb`；`alpha` 取 0–1，超出范围会被限制。无法解析的颜色会被
忽略。`dim` 也在这里：它叠加在 `[colors].dim` 之上，后者仍保留颜色字符串写法。

| 键 | 派生自 | 默认透明度 |
| --- | --- | --- |
| `card` | `container` | 0.72（暗色 0.85） |
| `selection` | `primary` | 0.15 |
| `hover` | `primary` | 0.08 |
| `preview` | `primary` | 0.05 |
| `hairline` | 浅色下黑色，暗色下白色 | 0.10（暗色 0.12） |
| `muted` | `fg` | 0.55 |
| `summary` | `fg` | 0.70 |
| `footer` | `fg` | 0.50 |
| `accent` | `primary` | 1.00 |
| `dim` | `#000000` | 0.30 |

`[surfaces]` 是共享层；`[surfaces.dark]` 与 `[surfaces.light]` 接受相同的键，
在该模式下按表面、按字段叠加。模式表可以只改颜色或只改透明度：

```toml
[surfaces]
hairline = { alpha = 0.12 }

[surfaces.dark]
hairline = { color = "#ffffff" }

[surfaces.light]
hairline = { color = "#000000" }
```

### `[blur]`

卡片是半透明的；合成器支持 `ext-background-effect-v1` 时，卡片背后的区域由合成器
模糊，磨砂观感即来源于此。合成器没有该协议时会忽略这个区域，此项也就没有可见效果。

在 niri 上还需要一条额外规则。niri 会为客户端通过 `ext-background-effect` 发起的
请求默认自动开启 xray：只模糊壁纸，忽略下方窗口。要让卡片模糊真正背后的内容，需要
加一条关闭 xray 的 layer rule：

```kdl
layer-rule {
    match namespace="WayRun"
    background-effect {
        xray false
    }
}
```

其中 namespace 是 `WayRun`。非 xray 模糊会在下方内容变化时重新计算，开销高于默认。

Hyprland 不实现 `ext-background-effect-v1`，会忽略这个区域；改用它自己的模糊，并按
namespace 匹配图层：

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

### `[preview]`

| 键 | 类型 | 默认值 | 含义 |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | 选中行时是否显示右侧分栏预览。`false` 时列表保持整宽，插件预览不显示。 |

### `[layout]`

| 键 | 类型 | 默认值 | 范围 | 含义 |
| --- | --- | --- | --- | --- |
| `width` | float | `730.0` | > 0 | 卡片宽度（逻辑像素）；输出更窄时按屏宽收缩。 |
| `radius` | float | `16.0` | ≥ 0 | 卡片圆角；`0` 即直角，内层圆角随之收缩。 |
| `top_ratio` | float | `0.20` | 0–1 | 卡片顶部占输出高度的比例。 |
| `align` | string | `center` | `left`/`center`/`right` | 在输出上的水平锚点。 |
| `max_rows` | int | `5` | 1–8 | 可见结果行数。 |
| `panel_header` | float | `28.0` | ≥ 0 | 动作面板中动作上方的标题栏高度。 |

`align` 无法识别时保留默认值。

### `[font]`

界面尺寸，逻辑像素。一个尺寸驱动全部角色，其余角色随之缩放。字体族不在这里，位于
`config.toml` 的 `[font].family`。

| 键 | 类型 | 默认值 | 范围 | 含义 |
| --- | --- | --- | --- | --- |
| `size` | float | `14.0` | 1–96 | 结果行标题；其余角色由它缩放。 |

### `[motion]`

| 键 | 类型 | 默认值 | 含义 |
| --- | --- | --- | --- |
| `reduced` | bool | `false` | 等价于 `WAYRUN_REDUCED_MOTION=1`：两段动画都直接停在终态。 |
