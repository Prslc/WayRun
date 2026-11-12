# Resident mode

By default each hotkey press starts the launcher from scratch. To pop it up
instantly, keep one resident process alive and toggle its window over a unix
socket at `$XDG_RUNTIME_DIR/wayrun.sock`.

## systemd unit

```ini
# ~/.config/systemd/user/wayrun-launcher.service
[Unit]
Description=WayRun launcher (resident)
After=graphical-session.target
PartOf=graphical-session.target

[Service]
Type=simple
ExecStart=/home/you/.local/bin/wayrun run
# dismissing the launcher is a clean exit, so "on-failure" would be wrong
Restart=always
RestartSec=2
# no PATH here: the unit inherits the systemd user manager's environment
# WAYLAND_DISPLAY/DISPLAY come from the graphical session (imported by the
# compositor); only XDG_RUNTIME_DIR is set, via the uid-proof `%t` specifier
Environment=XDG_RUNTIME_DIR=%t
# resident mode: start hidden, toggle via `wayrun toggle`
Environment=WAYRUN_RESIDENT=1

[Install]
WantedBy=default.target
```

### Environment

The unit inherits the systemd user manager's environment. The commands the
launcher runs therefore see the same `PATH` as your other user services.

To add a directory, such as `~/.cargo/bin`, put it in `~/.config/environment.d/`.
Do not set `PATH=` in the unit: a unit-level assignment replaces the whole
inherited value.

A plugin that fetches over HTTP reads these variables. To set a proxy with
`Environment=`, see [Network and proxy](plugins.md#network-and-proxy).

## Enable and bind a hotkey

Bind `wayrun toggle` in your compositor's config. Any compositor that can run a
command on a key works; niri and Hyprland are two examples:

```sh
systemctl --user enable --now wayrun-launcher
# niri hotkey — toggle instead of spawn:
#   Alt+Space { spawn-sh "wayrun toggle"; }
# Hyprland hotkey:
#   bind = SUPER, SPACE, exec, wayrun toggle
```

## Control commands

The resident instance owns the socket and answers two commands: `toggle` and
`status`. `status` prints `visible` or `hidden` on the first line, then
`desktop: <id>`, `pid: <pid>`, `socket: <path>` and `version: <version>`;
`desktop: unknown` means no variable names the session.

Without `WAYRUN_RESIDENT=1`, `wayrun run` shows on launch and quits on dismiss.
A bare `wayrun` prints the command list in a terminal. Spawned without one, by a
keybind, a unit or a script, it runs the launcher.

## Launched applications

Apps you launch run in their own transient systemd scope
(`systemd-run --user --scope`). The unit's memory and CPU therefore stay the
launcher's own. A restart never touches a running app, and `systemd-oomd` can
pick a single app instead of the whole launcher.

Without `systemd-run` or a reachable user manager, launches fall back to plain
detached processes.

## Reverting

Run `systemctl --user disable --now wayrun-launcher` and restore the
spawn-per-hotkey binding.
