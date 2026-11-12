# Plugin directory

These examples live in the
[WayRun-Plugins](https://github.com/Prslc/WayRun-Plugins) workspace. They are
reference material: nothing is registered until your `plugins.toml` names it.
Most Lua examples are one script; `kde.lua` and `windows.lua` carry their
`kde/` and `windows/` modules. A Python example is a directory that carries
its own `wayrun_plugin/` copy.

## Lua examples

| Plugin | What it does | Needs |
| --- | --- | --- |
| [Firefox](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/firefox.lua) | Bookmarks and history from `places.sqlite` | a Firefox profile; `assets/` |
| [Web search](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/web.lua) | Search-engine suggestions | network; `assets/` |
| [Clipboard history](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/cliphist.lua) | Search `cliphist`, copy an entry back | `cliphist`; `wl-clipboard` |
| [Screenshots](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/screenshots.lua) | Recent screenshots with image previews | `~/Pictures/Screenshots`, or edit `DIR` and `read` |
| [Notes](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/documents.lua) | Text and Markdown notes with previews | `documents/` |
| [GitHub](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/github.lua) | Repository and user search | network; optional `GITHUB_TOKEN` |
| [Wallpaper](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/wallpaper_dms.lua) | Switch the DMS wallpaper | `dms` on `PATH`; `~/Pictures/Wallpapers` |
| [Web open](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/web-open.lua) | Open a typed URL or domain as a default provider | — |
| [Man pages](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/man.lua) | Search man pages and open one in a terminal | `apropos` (man-db) |
| [Currency](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/currency.lua) | Convert between currencies at daily rates | network |
| [KDE settings](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/kde.lua) | Search KDE System Settings | KDE Plasma; `systemsettings` or `kcmshell6` |
| [Packages](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/packages.lua) | Search pacman repositories and the AUR | Arch; `pacman`; `paru` or `yay` optional |
| [Windows](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/windows.lua) | Switch between open windows | KDE, niri or Hyprland; `busctl`, `niri` or `hyprctl` |
| [Kill process](https://github.com/Prslc/WayRun-Plugins/blob/main/examples/lua/kill.lua) | Find a process by name and send SIGTERM or SIGKILL | `ps` and `kill` (procps-ng) |

`firefox.lua` and `web.lua` read the `assets/` folder beside the script;
`documents.lua` reads `documents/`, `kde.lua` reads `kde/`, and `windows.lua`
reads `windows/`. Copy the folder with the script.

## Python examples

| Plugin | What it does | Needs |
| --- | --- | --- |
| [GitHub](https://github.com/Prslc/WayRun-Plugins/tree/main/examples/python/github) | Repository search | Python 3; framework included |
| [Todo](https://github.com/Prslc/WayRun-Plugins/tree/main/examples/python/todo) | Todo manager with a panel action | Python 3; framework included |

## Installing an example

For a Lua plugin, clone the workspace and register the script. `plugin add`
refuses a script without the exec bit, and the keyword is yours to pick:

```bash
git clone https://github.com/Prslc/WayRun-Plugins.git
chmod +x WayRun-Plugins/examples/lua/firefox.lua
wayrun plugin add WayRun-Plugins/examples/lua/firefox.lua --keyword ff
```

For a Python plugin, add a `plugins.toml` entry yourself; `plugin add` registers
Lua scripts only. The example carries its own `wayrun_plugin/`, so `command`
can point straight at `main.py`:

```toml
[[plugins]]
id = "github"
keyword = "gh"
command = "/absolute/path/to/examples/python/github/main.py"
```

The script needs its exec bit; run `chmod +x main.py` if you copied it. Check a
Lua example with `wayrun plugin check <script.lua>`. The workspace's
[Python guide](https://github.com/Prslc/WayRun-Plugins/blob/main/docs/en/PYTHON.md)
covers the framework and its API.

These are examples, not reviewed packages: read a script before registering it.
