# Resident mode

By default each hotkey press starts the launcher from scratch. To pop it up
instantly, keep one resident process alive and toggle its window over a unix
socket (`$XDG_RUNTIME_DIR/wayrun.sock`):

```ini
# ~/.config/systemd/user/wayrun-launcher.service
[Unit]
Description=WayRun launcher (resident)
After=graphical-session.target
PartOf=graphical-session.target

[Service]
Type=simple
ExecStart=/home/you/.local/bin/wayrun
# dismissing the launcher is a clean exit, so "on-failure" would be wrong
Restart=always
RestartSec=2
# so commands the launcher runs see ~/.local/bin too
Environment=PATH=/home/you/.local/bin:/usr/local/bin:/usr/bin:/bin
# WAYLAND_DISPLAY/DISPLAY come from the graphical session (imported by the
# compositor); only XDG_RUNTIME_DIR is set, via the uid-proof `%t` specifier
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
resident instance owns; `status` prints `visible` or `hidden`. Without
`WAYRUN_RESIDENT=1`, a plain `wayrun` shows on launch and quits on dismiss. To
revert, `systemctl --user disable --now wayrun-launcher` and restore the
spawn-per-hotkey binding.

Apps you launch run in their own transient systemd scope
(`systemd-run --user --scope`), so the unit's memory and CPU stay the launcher's
own, a restart never touches a running app, and `systemd-oomd` can pick a single
app instead of the whole launcher. Without `systemd-run` or a reachable user
manager, launches fall back to plain detached processes.

## Proxy

The core makes no outbound requests itself; a registered host that fetches does
— the WayRun-Plugins `web.lua`, for instance. Its `wayrun.http.get` reads the
launcher's environment, so configure a proxy with `Environment=` in the unit:

```ini
# HTTP CONNECT proxy; scheme optional, credentials optional
Environment=https_proxy=http://127.0.0.1:7890
Environment=http_proxy=http://127.0.0.1:7890
```

Then `systemctl --user daemon-reload && systemctl --user restart wayrun-launcher`.
The lowercase variable is read first, then the uppercase one, as curl reads
them; the chosen proxy is used for every request. Only HTTP CONNECT proxies are
supported (`[http://][user[:password]@]host[:port]`, default port 1080); a
`socks5://` URL is not, and an unusable value is ignored, so the request falls
back to a direct connection.
