# 常驻模式

默认每次按热键都会从头拉起启动器。要让启动器即刻弹出，让一个常驻进程保持存活，
通过 unix socket（`$XDG_RUNTIME_DIR/wayrun.sock`）切换窗口：

```ini
# ~/.config/systemd/user/wayrun-launcher.service
[Unit]
Description=WayRun launcher (resident)
After=graphical-session.target
PartOf=graphical-session.target

[Service]
Type=simple
ExecStart=/home/you/.local/bin/wayrun
# 关闭启动器属于正常退出，因此不能用 on-failure
Restart=always
RestartSec=2
# 让启动器执行的命令也能看到 ~/.local/bin
Environment=PATH=/home/you/.local/bin:/usr/local/bin:/usr/bin:/bin
# WAYLAND_DISPLAY/DISPLAY 由图形会话导入；这里只设 XDG_RUNTIME_DIR（uid 无关的 %t）
Environment=XDG_RUNTIME_DIR=%t
# 常驻模式：隐藏启动，用 `wayrun toggle` 切换
Environment=WAYRUN_RESIDENT=1

[Install]
WantedBy=default.target
```

在合成器配置里绑定 `wayrun toggle` 即可；凡是能在按键上执行命令的合成器都可以，
下面仅以 niri 与 Hyprland 举例：

```sh
systemctl --user enable --now wayrun-launcher
# niri 热键 —— 切换而非重新拉起：
#   Alt+Space { spawn-sh "wayrun toggle"; }
# Hyprland 热键：
#   bind = SUPER, SPACE, exec, wayrun toggle
```

命令为 `open` / `close` / `toggle` / `status`，都发给常驻实例持有的这个 socket；
`status` 打印 `visible` 或 `hidden`。不带 `WAYRUN_RESIDENT=1` 时，直接 `wayrun` 启动即弹出、
关闭即退出。恢复：`systemctl --user disable --now wayrun-launcher` 并还原绑定的启动方式。

启动器打开的应用各自运行在自己的瞬时 systemd scope 中（`systemd-run --user --scope`）：
单元的 memory/CPU 只统计启动器自身，重启不会波及正在运行的应用，
`systemd-oomd` 也能只挑某个应用而不是整个启动器。环境里没有 `systemd-run`
或可用的 user manager 时，回退为直接拉起的独立进程。

## 代理

后端自身不发起对外请求；需要联网的是注册的主机（例如 WayRun-Plugins 的
`web.lua`）。它的 [`wayrun.http.get`](lua.md#wayrun-表) 读取启动器进程的环境变量，
因此在单元文件里用 `Environment=` 指定：

```ini
# HTTP CONNECT 代理；账号密码可省略
Environment=https_proxy=http://127.0.0.1:7890
Environment=http_proxy=http://127.0.0.1:7890
```

改完执行 `systemctl --user daemon-reload && systemctl --user restart wayrun-launcher`。
依次读取 `ALL_PROXY`、`HTTPS_PROXY`、`HTTP_PROXY`（每个变量大小写两种写法都认），
第一个可用值生效，不可用的值跳过而不是中断查找，坏掉的 `https_proxy` 不会再挡
住可用的 `http_proxy`；`NO_PROXY` 按主机名匹配（精确、后缀、通配符三种形式）。
scheme 要写出来：`http://` 是端口 80 的 HTTP CONNECT 代理，`https://` 是 443，
省略 scheme 会被当作 HTTP 而不是 SOCKS；`socks5://` 在发行版里不支持。
没有任何可用值时请求直连。
