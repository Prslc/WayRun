# 配置

`$XDG_CONFIG_HOME/wayrun/config.toml`（默认 `~/.config/wayrun/config.toml`）保存
WayRun 的设置。首次运行会生成一份带注释的模板，并监听该文件；修改后无需重启即可生效。
所有键都是可选的：文件或键缺失时，使用下面列出的内置默认值。

## `[ui]`

| 键 | 默认 | 含义 |
| --- | --- | --- |
| `locale` | *（会话语言）* | 界面语言：`en` 或 `zh_cn`。省略时跟随 glib 的 `$LANGUAGE`/`$LC_ALL`/`$LC_MESSAGES`/`$LANG`。 |

启动时读取，修改后需下次启动生效（`systemctl --user restart wayrun-launcher`）。
`.desktop` 文件带来的文字（应用名称、说明与桌面动作名）也跟随此设置。没有对应语言表
的值回退到英文；留空等同不写这个键。

## `[font]`

| 键 | 默认 | 含义 |
| --- | --- | --- |
| `family` | `sans-serif` | 主文本字体族，或通用名（`serif`、`sans-serif`、`monospace`）。 |

启动时读取，修改后需下次启动生效（`systemctl --user restart wayrun-launcher`）。
字体族缺少的字形会回退到系统字体。界面字号在 `theme.toml` 的 `[font].size`，可实时
生效。

## `[icon]`

| 键 | 默认 | 含义 |
| --- | --- | --- |
| `theme` | *（桌面设置）* | 用于结果行图标的图标主题。省略时使用桌面自身的图标主题。 |

图标主题是可选的：面板与内置插件的图形都已编入二进制；某行图标在所有主题里都找不到
时，回退到内置占位图。设置 `theme` 时它优先于桌面设置。

## `[files]`

| 键 | 默认 | 含义 |
| --- | --- | --- |
| `index` | `true` | 让 `f` 与 `d` 能命中 `$HOME` 任意位置的文件与目录，而不只是靠近顶层的部分。 |
| `exclude` | `["node_modules", "target", "__pycache__"]` | 搜索不进入、结果也不显示的目录名。以 `.` 开头的名称（如 `.git`、`.cache`）一律隐藏，列表里也不会出现。默认值中的名字以注释形式写在 `config.toml` 里；自己写一份列表会整体替换它，`exclude = []` 则除隐藏名称外全部搜索。 |

索引在后台建立，首次使用 `f` 或 `d` 时才开始。设 `index = false` 时只搜索 `$HOME`
下几层，也不留下缓存；禁用这两个插件效果相同。修改 `exclude` 后，索引会在下一次
`f`/`d` 搜索时重建。隐藏或已排除的名称仍可通过**输入路径**命中（如
`~/.config/niri/config.kdl`、`.config/niri/`）。
