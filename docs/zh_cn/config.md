# 配置

`$XDG_CONFIG_HOME/wayrun/config.toml`（默认 `~/.config/wayrun/config.toml`）保存 WayRun 的设置。首次运行会写入一份带注释的模板，启动器会监听该文件，修改后无需重启即可生效。所有键都是可选的；文件或键缺失时，使用这里列出的内置默认值。

## 从命令行编辑

`wayrun config list` 打印全部生效设置（默认值与你的文件合并后的结果），`wayrun config get <key>` 打印其中一项。

`wayrun config set <key> <value>` 与 `wayrun config unset <key>` 直接修改文件。两者都会保留文件里的注释，并在写入前重新解析校验。键用点号分隔，如 `ui.locale`、 `files.exclude`。文本值通常不用加引号，除非它会被解析成别的 TOML 类型：

```bash
wayrun config set files.index false
wayrun config set files.exclude '["node_modules", "target"]'
```

`locale` 在启动时读取，修改后需要执行 `systemctl --user restart wayrun-launcher`。其余键（包括 `font.family`）通过监听器即时生效。

## 界面语言

`[ui]` 表只有一个键。

| 键 | 默认 | 含义 |
| --- | --- | --- |
| `locale` | *（会话语言）* | 界面语言：`en` 或 `zh_cn`。省略时跟随 glib 的 `$LANGUAGE` / `$LC_ALL` / `$LC_MESSAGES` / `$LANG`。 |

启动器只在启动时读取一次界面语言，顺序与 glib 一致。先取 `$LANGUAGE` 中第一个有文案表的项，再依次取 `$LC_ALL`、`$LC_MESSAGES`、`$LANG`。`C` 或 `POSIX` 这类基础 locale 会忽略 `$LANGUAGE`。目前内置英文与简体中文，其他语言一律回退到英文。

`[ui] locale` 可以让语言不随会话变化。常驻服务尤其需要它：服务单元要么继承会话环境，要么什么都不继承。在单元里设置 `Environment=LC_ALL=zh_CN.UTF-8` 同样有效。

这个选择也决定 `.desktop` 文件带来的文字用哪种语言：应用名称、说明与桌面动作名。没有对应文案表的值回退到英文；留空等同不写这个键。

两种方式都只在启动时读取，修改后需要重启服务。

## 字体

`[font]` 表设置文本字体族。

| 键 | 默认 | 含义 |
| --- | --- | --- |
| `family` | `sans-serif` | 主文本字体族，或通用名（`serif`、`sans-serif`、`monospace`）。 |

文件变化后，启动器会立即用新的字体族重新排版。字体族缺少的字形回退到系统字体。界面字号在 `theme.toml` 的 `[font].size`，同样即时生效。

## 图标主题

`[icon]` 表选择图标主题。

| 键 | 默认 | 含义 |
| --- | --- | --- |
| `theme` | *（桌面设置）* | 用于结果行图标的图标主题。省略时使用桌面自身的图标主题。 |

图标主题是可选的。面板与内置插件的字形都已编入二进制；某行图标在所有主题里都找不到时，回退到内置占位图。设置 `theme` 时它优先于桌面设置。

## 文件与索引

`[files]` 表控制文件与目录搜索。

| 键 | 默认 | 含义 |
| --- | --- | --- |
| `index` | `true` | 让 `f` 与 `d` 能命中 `$HOME` 下方任意位置的文件与目录，而不只是靠近顶层的部分。 |
| `exclude` | `["node_modules", "target", "__pycache__"]` | 搜索不进入、结果也不显示的目录名。以 `.` 开头的名称（`.git`、`.cache`）一律隐藏，也不会重新出现在结果里。默认值中的名字已编入二进制，并以注释形式写在 `config.toml` 里；自己写一份列表会整体替换它，`exclude = []` 则除隐藏名称外全部搜索。 |

### 索引如何工作

索引在后台建立，首次使用 `f` 或 `d` 时才启动。设 `index = false` 后只搜索 `$HOME` 下几层，也不留下缓存；禁用这两个插件效果相同。修改 `exclude` 后，索引会在下一次 `f`/`d` 搜索时重建。

### 隐藏与排除的名称

索引里不会出现隐藏或排除的名称，因此 `f` 与 `d` 找不到它们。直接输入路径仍然可以打开，例如 `~/.config/niri/config.kdl` 或 `.config/niri/`。
