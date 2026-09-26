# 配置

`~/.config/wayrun/config.toml` 保存 WayRun 的设置。首次运行时自动生成一份带注释的模板，
并且会被监听，修改后无需重启即可生效。所有键都是可选的：文件或键缺失即使用此处列出的
内置默认值。

## `[ui]`

| 键 | 默认 | 含义 |
| --- | --- | --- |
| `locale` | *（会话语言）* | 界面语言：`en` 或 `zh_cn`。省略时跟随 `$LC_ALL`/`$LC_MESSAGES`/`$LANG`/`$LANGUAGE`。 |

启动时读取，因此修改后需下次启动生效（`systemctl --user restart wayrun-launcher`）。
`.desktop` 文件带来的文字——应用名称、说明与桌面动作名——同样跟随此设置。
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

## `[files]`

| 键 | 默认 | 含义 |
| --- | --- | --- |
| `index` | `true` | 让 `f` 与 `d` 能命中 `$HOME` 任意位置的文件与目录，而不只是靠近顶层的部分。 |
| `exclude` | `["node_modules", "target", "__pycache__"]` | 搜索不进入、结果也不显示的目录名。以 `.` 开头的名称（`.git`、`.cache`）一律隐藏，列出也无法找回。随附的名字编译在默认值里，并在 `config.toml` 中以注释形式给出；自己写一份列表会整体替换它，`exclude = []` 则除隐藏名称外全部搜索。 |

索引在后台建立，且只在你使用 `f` 或 `d` 之后才开始。设 `index = false` 则只搜索 `$HOME`
下几层，且不留下缓存；把这两个插件都禁用也是同样效果。修改 `exclude` 后，
索引会在下一次 `f`/`d` 搜索时重建。隐藏或已排除的名称仍可通过**输入路径**命中
（`~/.config/niri/config.kdl`、`.config/niri/`）。
