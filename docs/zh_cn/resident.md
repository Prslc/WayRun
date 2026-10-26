# 常驻模式

默认每次按热键都会重新拉起启动器。要让启动器立即弹出，可以让一个常驻进程保持存活，
通过 Unix socket（`$XDG_RUNTIME_DIR/wayrun.sock`）切换窗口：

```ini
# ~/.config/systemd/user/wayrun-launcher.service
[Unit]
Description=WayRun launcher (resident)
After=graphical-session.target
PartOf=graphical-session.target

[Service]
Type=simple
ExecStart=/home/you/.local/bin/wayrun run
# 关闭启动器属于正常退出，因此不能用 on-failure
Restart=always
RestartSec=2
# 不在这里设 PATH：单元继承 systemd 用户管理器的环境
# WAYLAND_DISPLAY/DISPLAY 由图形会话导入；这里只设 XDG_RUNTIME_DIR（uid 无关的 %t）
Environment=XDG_RUNTIME_DIR=%t
# 常驻模式：隐藏启动，用 `wayrun toggle` 切换
Environment=WAYRUN_RESIDENT=1

[Install]
WantedBy=default.target
```

单元继承 systemd 用户管理器的环境，启动器执行的命令与其他用户服务看到同一个 `PATH`。
要补充目录（例如 `~/.cargo/bin`），写进 `~/.config/environment.d/`，不要在单元里设
`PATH=`：单元里的赋值会整体替换继承来的值。

在合成器配置里绑定 `wayrun toggle` 即可；任何能在按键上执行命令的合成器都行，下面
以 niri 与 Hyprland 为例：

```sh
systemctl --user enable --now wayrun-launcher
# niri 热键 —— 切换而非重新拉起：
#   Alt+Space { spawn-sh "wayrun toggle"; }
# Hyprland 热键：
#   bind = SUPER, SPACE, exec, wayrun toggle
```

命令 `open` / `close` / `toggle` / `status` 都发给常驻实例持有的这个 socket；`status`
打印 `visible` 或 `hidden`。不设 `WAYRUN_RESIDENT=1` 时，`wayrun run` 会弹出、关闭即
退出；在终端里裸 `wayrun` 打印命令列表，而没有终端的调用（热键、unit、脚本）仍然运行
启动器。恢复：`systemctl --user disable --now wayrun-launcher`，并还原绑定的启动方式。

启动器打开的应用各自运行在自己的瞬时 systemd scope 中（`systemd-run --user --scope`）：
单元的内存与 CPU 统计只算启动器自身，重启不会影响正在运行的应用，`systemd-oomd`
也可以只挑某个应用，而不是整个启动器。环境里没有 `systemd-run` 或可用的 user manager
时，回退为直接启动的独立进程。

## 代理

后端自身不发起对外请求；需要联网的是注册的主机（例如 WayRun-Plugins 的 `web.lua`）。
主机通过 [`wayrun.http.get`](lua.md#wayrun-表) 读取启动器进程的环境变量，因此在服务
单元里用 `Environment=` 指定：

```ini
# HTTP CONNECT 代理；账号密码可省略
Environment=https_proxy=http://127.0.0.1:7890
Environment=http_proxy=http://127.0.0.1:7890
```

改完执行 `systemctl --user daemon-reload && systemctl --user restart wayrun-launcher`。
依次读取 `ALL_PROXY`、`HTTPS_PROXY`、`HTTP_PROXY`（大小写两种写法都认），第一个可用
值生效；不可用的值会被跳过，不会中断查找，坏掉的 `https_proxy` 不会再挡住可用的
`http_proxy`。`NO_PROXY` 按主机名匹配，支持精确、后缀与通配符三种形式。scheme 必须
写出：`http://` 是端口 80 的 HTTP CONNECT 代理，`https://` 是 443；省略 scheme 会按
HTTP 而不是 SOCKS 处理；`socks5://` 在发行版里不支持。没有任何可用值时请求直连。
