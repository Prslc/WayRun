# Resident mode

By default each hotkey press re-spawns the shell and its Rust core. To pop the
launcher up instantly, keep one resident process alive and toggle the surface
over a unix socket (`$XDG_RUNTIME_DIR/wayrun.sock`):

```ini
# ~/.config/systemd/user/wayrun-launcher.service
[Unit]
Description=WayRun launcher (resident)
After=graphical-session.target
PartOf=graphical-session.target

[Service]
Type=simple
ExecStart=/home/you/.local/bin/wayrun
# the shell exits 0 on a core crash, which is a *clean* exit — never "on-failure"
Restart=always
RestartSec=2
# so commands the launcher runs see ~/.local/bin too
Environment=PATH=/home/you/.local/bin:/usr/local/bin:/usr/bin:/bin
# WAYLAND_DISPLAY/DISPLAY come from the graphical session (imported by the
# compositor); only XDG_RUNTIME_DIR is set, via the uid-proof `%t` specifier —
# no hardcoded display number or uid.
Environment=XDG_RUNTIME_DIR=%t
# resident mode: start hidden, toggle via `wayrun toggle`
Environment=WAYRUN_RESIDENT=1

[Install]
WantedBy=default.target
```

Bind `wayrun toggle` in your compositor's config. Any compositor that can run
a command on a key works; niri and Hyprland are just the two shown here:

```sh
systemctl --user enable --now wayrun-launcher
# niri hotkey — toggle instead of spawn:
#   Alt+Space { spawn-sh "wayrun toggle"; }
# Hyprland hotkey:
#   bind = SUPER, SPACE, exec, wayrun toggle
```

The commands are `open` / `close` / `toggle` / `status`, spoken to the socket the
resident instance owns; `status` prints `visible` or `hidden`. `WAYRUN_RESIDENT=1`
selects resident mode (start hidden, dismiss hides); without it a plain `wayrun`
shows on launch and quits on dismiss, so the manual/dev path is independent of
the systemd service. To revert, `systemctl --user disable --now wayrun-launcher`
and restore the spawn-per-hotkey binding.

## Proxy

The launcher's only outbound request is the web-search suggestion fetch, so a
proxy only matters if the engine (`s` by default) is unreachable directly. Point
the core at one with `Environment=` in the unit:

```ini
# HTTP CONNECT proxy; scheme optional, credentials optional
Environment=https_proxy=http://127.0.0.1:7890
Environment=http_proxy=http://127.0.0.1:7890
```

Then `systemctl --user daemon-reload && systemctl --user restart wayrun-launcher`.
`https_proxy` is what the (HTTPS) suggest endpoints use; `http_proxy` covers a
plain-`http://` engine. Each name is read case-insensitively, lowercase first
(`https_proxy` before `HTTPS_PROXY`), and an empty or unusable value is ignored,
so the request falls back to a direct connection. Only HTTP CONNECT proxies are
supported (`[http://][user[:password]@]host[:port]`, default port 1080); a
`socks5://` URL is not, and is logged to the core's stderr.
