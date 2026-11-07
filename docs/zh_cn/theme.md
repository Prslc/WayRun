# 主题

WayRun 的配色分两层：桌面提供的系统调色板，以及 `$XDG_CONFIG_HOME/wayrun/theme.toml`（默认 `~/.config/wayrun/theme.toml`）里的覆盖值。启动器同时监听两者，修改后即时生效。

## 系统调色板

调色板跟随会话。

### KDE Plasma

KDE Plasma 下，调色板取自 `~/.config/kdeglobals` 中的当前配色方案。KDE 会把已应用方案的各个颜色角色合并进该文件，并把显式或取自壁纸的强调色写回 `[General] AccentColor`。

这里没有模式键。窗口背景的亮度决定当前是浅色还是深色，因此切换模式会即时生效。

| WayRun | KDE，来自 `kdeglobals` |
| --- | --- |
| `mode` | `[Colors:Window] BackgroundNormal` 的亮度 |
| `primary` | `[General] AccentColor`，否则 `[Colors:Window] DecorationFocus` |
| `fg` | `[Colors:Window] ForegroundNormal` |
| `container` | `[Colors:Button] BackgroundNormal` |
| `bg` | `[Colors:Window] BackgroundNormal` |

### DankMaterialShell

DankMaterialShell 桌面下，调色板是 DMS 用 matugen 生成的 Material You 配色，位于 `~/.cache/DankMaterialShell/dms-colors.json`。文件同时保存浅色与深色调色板以及当前模式。切换明暗或更换壁纸都会即时生效。

| WayRun | DMS，来自 `colors[mode]` |
| --- | --- |
| `primary` | `primary` |
| `fg` | `on_surface` |
| `container` | `surface_container_high` |

### 优先级与回退

KDE 会话只读 `kdeglobals`，即使别的桌面留下了 DMS 文件。两个来源都不存在时，使用内置深色调色板，`mode = "dark"`。

## 主题覆盖

所有小节与键都是可选的。缺失的键保留默认值；无法解析的文件被忽略；越界值会被限制到范围内。首次运行会写入一份带注释的模板，只列出选项，不固定当前默认值。只取消注释你想改的部分。

### 颜色

`[colors]` 表设置三个基础颜色角色，加上背景遮罩。颜色角色的写法为 `#rrggbb` 或 `#rgb`。`dim` 还接受 `#rrggbbaa` 与 `#rgba`，末尾两位是 alpha 值（0 全透明，1 不透明）。无法解析的值会被跳过，默认值保留。

| 键 | 默认 | 含义 |
| --- | --- | --- |
| `primary` | 系统调色板 | 强调色：选中底色、强调条、面板标题。 |
| `fg` | 系统调色板 | 文字、图标与提示的颜色。 |
| `container` | 系统调色板 | 卡片底色。 |
| `dim` | `#0000004d` | 背景遮罩；alpha 已包含在颜色值里。 |

`[colors]` 是共享的一套；`[colors.dark]` 与 `[colors.light]` 按模式覆盖，见下文。

其余表面都由颜色角色按固定的 alpha 值派生。例如卡片底色是 `container` 的 0.72（深色模式 0.85），选中行底色是 `primary` 的 0.15。完整列表见 `[surfaces]` 表。

未设置的键继续跟随系统调色板，matugen 或 DMS 的改动仍会重新着色；设置某个键就会固定该值。设置过的颜色角色不再跟随调色板，未设置的颜色角色继续跟随。整个小节不写，等同于纯动态主题。

#### 按模式覆盖

手动选择的颜色是固定的，而系统调色板会在明暗之间切换。`[colors.dark]` 与 `[colors.light]` 接受与 `[colors]` 相同的键，在当前模式生效时叠加在它上面。只在某一模式可用的颜色因此可以单独不同，其余仍共享：

```toml
[colors]
primary = "#7aa2f7"          # 两种模式共享

[colors.dark]
fg = "#c0caf5"

[colors.light]
fg = "#1f2430"
```

当前模式跟随系统调色板（没有来源时为 `dark`），所以切换模式无需重启。当前模式表里没有设置的键，沿用 `[colors]` 的值。

### 表面

`[surfaces]` 表用于固定渲染器从基础颜色角色派生的各个表面。条目可以设置 `color`、 `alpha` 或两者；省略的字段继续派生，因此 `[colors]` 的改动或调色板的实时变化仍会影响它。

```toml
[surfaces]
card = { alpha = 0.90 }                 # 继续跟随 container，更不透明
selection = { color = "#7aa2f7", alpha = 0.20 }
hairline = { color = "#ffffff" }        # 保留当前模式的默认 alpha
```

`color` 为 `#rrggbb` 或 `#rgb`；`alpha` 为 0–1，越界会被限制到范围内。无法解析的颜色被忽略。`hairline` 还接受逻辑像素单位的 `width`（0–16，默认 1.0；`0` 隐藏边缘），叠加方式与 `color`、`alpha` 相同。`dim` 也在这里：它叠加在 `[colors].dim` 之上，后者保留纯字符串写法。

| 键 | 派生自 | 默认 alpha |
| --- | --- | --- |
| `card` | `container` | 0.72（深色 0.85） |
| `selection` | `primary` | 0.15 |
| `hover` | `primary` | 0.08 |
| `preview` | `primary` | 0.05 |
| `hairline` | 浅色模式黑、深色模式白 | 0.10（深色 0.12） |
| `muted` | `fg` | 0.55 |
| `summary` | `fg` | 0.70 |
| `footer` | `fg` | 0.50 |
| `accent` | `primary` | 1.00 |
| `dim` | `#000000` | 0.30 |

`[surfaces]` 是共享表。`[surfaces.dark]` 与 `[surfaces.light]` 接受相同的键，在当前模式生效时按表面、按字段叠加。一个模式可以只改颜色、只改 alpha，或只改宽度：

```toml
[surfaces]
hairline = { alpha = 0.12 }

[surfaces.dark]
hairline = { color = "#ffffff", width = 1.5 }

[surfaces.light]
hairline = { color = "#000000" }
```

### 模糊

`[blur]` 表声明模糊区域。卡片是半透明的。合成器支持 `ext-background-effect-v1` 时，会模糊卡片背后的区域，磨砂效果由此而来。不支持该协议的合成器会忽略这块区域，此设置也就没有可见效果。

#### niri

niri 还需要一步。对客户端的 `ext-background-effect` 请求，niri 会自动开启 **xray**；xray 模糊壁纸，并忽略下面的窗口。要让卡片模糊它真正背后的内容，加一条关闭 xray 的 layer 规则：

```kdl
layer-rule {
    match namespace="WayRun"
    background-effect {
        xray false
    }
}
```

命名空间是 `WayRun`。非 xray 的模糊会在下方内容变化时重新计算，因此比默认方式开销更大。

#### Hyprland

Hyprland 不实现 `ext-background-effect-v1`，会忽略这块区域。打开它自己的模糊，再按命名空间模糊该层：

```ini
decoration {
    blur {
        enabled = true
    }
}
layerrule = blur, WayRun
```

#### KWin

KWin 实现了 `ext-background-effect-v1`。打开 Blur 桌面效果后，Plasma 就会模糊卡片，不需要 layer 规则。启动器映射到 `utility` 命名空间，KWin 的打开/关闭动画会跳过它。

| 键 | 类型 | 默认 | 含义 |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | 声明模糊区域。`false` 不声明任何区域，背景保持清晰；卡片本身的半透明底色不变。 |

### 预览

`[preview]` 表只有一个键。

| 键 | 类型 | 默认 | 含义 |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | 为选中行显示分栏预览。`false` 让列表占满宽度，并忽略插件提供的预览。 |

### 页脚

`[footer]` 表只有一个键。

| 键 | 类型 | 默认 | 含义 |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | 在结果下方显示提示栏：按键、结果数量与搜索状态。`false` 隐藏提示栏，把它占用的高度还给卡片。 |

### 布局

`[layout]` 表设置卡片几何。

| 键 | 类型 | 默认 | 范围 | 含义 |
| --- | --- | --- | --- | --- |
| `width` | float | `730.0` | > 0 | 卡片宽度，单位为逻辑像素；输出较窄时会缩小以适配。 |
| `radius` | float | `16.0` | ≥ 0 | 卡片圆角；`0` 为直角，内部各圆角随之缩小。 |
| `top_ratio` | float | `0.20` | 0–1 | 卡片顶部位置，占输出高度的比例。 |
| `align` | string | `center` | `left`/`center`/`right` | 在输出上的水平锚点。 |
| `max_rows` | int | `5` | 1–8 | 可见结果行数。 |
| `panel_header` | float | `28.0` | ≥ 0 | 动作面板标题区的高度，位于动作列表上方。 |

未知的 `align` 保留默认值。

### 字体

`[font]` 表设置界面字号，单位为逻辑像素。一个字号驱动所有文本层级，其余层级按它缩放。字体族不在这里，而在 `config.toml` 的 `[font].family`。

| 键 | 类型 | 默认 | 范围 | 含义 |
| --- | --- | --- | --- | --- |
| `size` | float | `14.0` | 1–96 | 结果行标题的字号；其他层级按它缩放。 |

### 动效

`[motion]` 表只有一个键。

| 键 | 类型 | 默认 | 含义 |
| --- | --- | --- | --- |
| `reduced` | bool | `false` | 等同于 `WAYRUN_REDUCED_MOTION=1`：两个动画都直接从结束状态开始。 |
