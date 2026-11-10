# 常驻模式

默认每次按热键都会从头启动启动器。要让窗口瞬间弹出，可以保留一个常驻进程，通过 Unix socket（`$XDG_RUNTIME_DIR/wayrun.sock`）切换窗口。

## systemd 单元

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

### 环境

单元继承 systemd 用户管理器的环境。启动器执行的命令与其他用户服务看到同一个 `PATH`。

要补充目录（例如 `~/.cargo/bin`），写进 `~/.config/environment.d/`。不要在单元里设 `PATH=`：单元里的赋值会整体替换继承来的值。

需要联网的插件会读取这些环境变量。要用 `Environment=` 设置代理，见[网络与代理](plugins.md#网络与代理)。

## 启用并绑定热键

在合成器配置里绑定 `wayrun toggle`。任何能在按键上执行命令的合成器都行，下面以 niri 与 Hyprland 为例：

```sh
systemctl --user enable --now wayrun-launcher
# niri 热键 —— 切换而非重新启动：
#   Alt+Space { spawn-sh "wayrun toggle"; }
# Hyprland 热键：
#   bind = SUPER, SPACE, exec, wayrun toggle
```

## 控制命令

常驻实例持有 socket，响应四个命令：`open`、`close`、`toggle`、`status`。`status` 第一行打印 `visible` 或 `hidden`，第二行打印 `desktop: <标识>`；无法识别会话时为 `desktop: unknown`。

不设 `WAYRUN_RESIDENT=1` 时，`wayrun run` 会弹出，关闭即退出。在终端里直接运行 `wayrun` 会打印命令列表；没有终端的调用（热键、systemd 单元、脚本）仍然运行启动器。

## 启动的应用

启动器打开的应用各自运行在自己的临时 systemd scope 中（`systemd-run --user --scope`）。单元的内存与 CPU 统计只算启动器自身。重启不会影响正在运行的应用，`systemd-oomd` 也可以只挑某个应用，而不是整个启动器。

环境里没有 `systemd-run`，或没有可用的 user manager 时，回退为直接启动的独立进程。

## 恢复

执行 `systemctl --user disable --now wayrun-launcher`，并还原绑定的启动方式。
