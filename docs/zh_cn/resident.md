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

## 代理

启动器对外只有一个请求：拉取网页搜索建议。所以只有搜索引擎（默认 `s`）没法直连时，
才需要配置代理。在单元文件里用 `Environment=` 指定：

```ini
# HTTP CONNECT 代理；scheme 和账号密码都可省略
Environment=https_proxy=http://127.0.0.1:7890
Environment=http_proxy=http://127.0.0.1:7890
```

改完执行 `systemctl --user daemon-reload && systemctl --user restart wayrun-launcher`。
建议接口走 HTTPS，对应 `https_proxy`；`http_proxy` 只在引擎用纯 `http://` 时生效。
目前只支持 HTTP CONNECT 代理（`[http://][user[:password]@]host[:port]`，端口默认 1080），
不支持 `socks5://`；无法使用的值会被忽略，请求退回直连。
