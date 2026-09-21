# 配置

`~/.config/wayrun/config.toml` 保存后端的行为设置。首次运行时自动生成（即
`core/default-config.toml` 的副本），并且会被监听，修改后无需重启后端即可生效。
所有键都是可选的：文件或键缺失即使用此处列出的内置默认值。生成的模板里所有键都被注释掉，
因此它把可选项列在字段旁，却不会把当前默认值钉死在文件里；只取消注释你想改的键即可。

## `[web_search]`

| 键 | 默认 | 含义 |
| --- | --- | --- |
| `engine` | `google` | 建议来源：`google` 或 `duckduckgo`。未知值回退到 `google`。 |

## `[font]`

| 键 | 默认 | 含义 |
| --- | --- | --- |
| `family` | `Source Han Sans CN` | 主文本整形字体族，或通用名（`serif`、`sans-serif`、`monospace`）。 |

前端在启动时读取 `family`，因此修改后需下次启动生效
（`systemctl --user restart wayrun-launcher`）。缺字会逐字回退到系统字体，因此只覆盖
你常用文字的字体族，能让从不绘制的字体不占用内存。界面字号在 `theme.toml` 的 `[font].size`，
可实时生效。

## `[icon]`

| 键 | 默认 | 含义 |
| --- | --- | --- |
| `theme` | *（桌面设置）* | 用于结果行图标的图标主题。省略时后端读取 GTK 设置的 `gtk-icon-theme-name`，并沿其 `Inherits` 链查找。 |

图标主题是可选的：面板、徽标与内置插件的图形都已编入二进制；某行图标在所有主题里都找不到时，
回退到内置占位图。设置 `theme` 时它优先于桌面设置。解析结果在后端进程内缓存，因此修改需下次
启动后端才生效。
