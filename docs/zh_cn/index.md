---
layout: home

hero:
  name: WayRun
  text: 无 GPU，无 GUI 框架
  tagline: Wayland 原生的 Linux 应用启动器与快速搜索工具
  image:
    src: /logo.svg
    alt: WayRun
  actions:
    - theme: brand
      text: 开始使用
      link: /zh_cn/install
    - theme: alt
      text: GitHub
      link: https://github.com/Prslc/WayRun

features:
  - title: 启动器
    details: 模糊搜索已安装应用，按桌面自身的方式启动，并逐行显示 Flatpak 应用与主题图标。
  - title: 快速搜索
    details: 文件与路径、$PATH 命令、系统命令与即时计算，共用同一段输入。
  - title: 动作面板
    details: Shift+Enter 打开按类型区分的动作面板：定位文件、运行 Desktop Action、复制链接。
  - title: 软件渲染
    details: 覆盖层由 tiny-skia 光栅化成预乘 RGBA 位图，再作为 wl_shm 缓冲交给合成器。
  - title: 可主题化
    details: 跟随 KDE 配色方案或 DankMaterialShell 的 Material You 调色板，并可用 theme.toml 覆盖配色、模糊、布局、字体与动效。
  - title: 可扩展
    details: 二进制内置 7 个插件，其余通过有文档的 JSON-RPC 契约作为外部宿主接入：可以用 Python 写，也可以就是一个由启动器本体运行的 Lua 脚本。
---
