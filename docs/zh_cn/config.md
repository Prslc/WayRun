# 配置

`~/.config/wayrun/config.toml` 保存 WayRun 的设置。首次运行时自动生成一份带注释的模板，
并且会被监听，修改后无需重启即可生效。所有键都是可选的：文件或键缺失即使用此处列出的
内置默认值。

## `[ui]`

| 键 | 默认 | 含义 |
| --- | --- | --- |
| `locale` | *（会话语言）* | 界面语言：`en` 或 `zh_cn`。省略时跟随 `$LC_ALL`/`$LC_MESSAGES`/`$LANG`。 |

启动时读取，因此修改后需下次启动生效（`systemctl --user restart wayrun-launcher`）。
没有对应语言表的值会退回英文，留空等同不写这个键。

## `[web_search]`

| 键 | 默认 | 含义 |
| --- | --- | --- |
| `engine` | `google` | 建议来源：`google` 或 `duckduckgo`。未知值回退到 `google`。 |

## `[font]`

| 键 | 默认 | 含义 |
| --- | --- | --- |
| `family` | `Source Han Sans CN` | 主文本整形字体族，或通用名（`serif`、`sans-serif`、`monospace`）。 |

启动时读取，因此修改后需下次启动生效（`systemctl --user restart wayrun-launcher`）。
字体族缺少的字形会回退到系统字体。界面字号在 `theme.toml` 的 `[font].size`，可实时生效。

## `[icon]`

| 键 | 默认 | 含义 |
| --- | --- | --- |
| `theme` | *（桌面设置）* | 用于结果行图标的图标主题。省略时使用桌面自身的图标主题。 |

图标主题是可选的：面板、徽标与内置插件的图形都已编入二进制；某行图标在所有主题里都找不到时，
回退到内置占位图。设置 `theme` 时它优先于桌面设置。修改需下次启动才生效。
