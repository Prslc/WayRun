---
layout: home

hero:
  name: WayRun
  text: Search, then launch.
  tagline: A Wayland-native application launcher and quick-search tool for Linux.
  image:
    src: /logo.svg
    alt: WayRun
  actions:
    - theme: brand
      text: Get started
      link: /en/install
    - theme: alt
      text: GitHub
      link: https://github.com/Prslc/WayRun

features:
  - title: Launcher
    details: Fuzzy search of installed applications, launched the way the desktop itself would, with Flatpak apps and themed icons per row.
  - title: Quick search
    details: Files and paths, $PATH commands, system commands, and inline math, all from the same input.
  - title: Action panel
    details: Shift+Enter opens a per-type action menu — reveal a file, run a desktop action, copy a link.
  - title: No GPU stack
    details: The overlay is rasterised in software into a shared-memory buffer. No GPU stack, no GUI toolkit.
  - title: Themeable
    details: Follows KDE's colour scheme or DankMaterialShell's Material You palette, with theme.toml overrides for colours, blur, layout, typography and motion.
  - title: Extensible
    details: Seven built-ins; everything else is an external host over a documented JSON-RPC contract, written in Python or as one Lua script the launcher runs itself.
---
