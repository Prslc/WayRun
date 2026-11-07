---
layout: page
head:
  - - script
    - {}
    - |
      (() => {
        const language =
          (navigator.languages && navigator.languages[0]) || navigator.language || ''
        const target = language.toLowerCase().startsWith('zh') ? 'zh_cn/' : 'en/'
        location.replace(target + location.search + location.hash)
      })()
---

# WayRun

This page redirects to the guide matching your browser language.

- [English](en/)
- [简体中文](zh_cn/)
