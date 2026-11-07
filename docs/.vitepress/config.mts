import { defineConfig } from 'vitepress'

const repo = 'https://github.com/Prslc/WayRun'
const base = '/WayRun/'

// The two guides share file names, which is also what the theme's language
// switch relies on to link a page to its counterpart.
const groups = [
  { key: 'start', en: 'Getting started', zh: '入门' },
  { key: 'configure', en: 'Configuration', zh: '配置' },
  { key: 'run', en: 'Running', zh: '运行' },
  { key: 'extend', en: 'Extending', zh: '扩展' },
] as const

type Group = (typeof groups)[number]['key']

const pages: { slug: string; en: string; zh: string; group: Group }[] = [
  { slug: 'install', en: 'Installation', zh: '安装', group: 'start' },
  { slug: 'usage', en: 'Usage', zh: '使用说明', group: 'start' },
  { slug: 'config', en: 'Config', zh: '配置', group: 'configure' },
  { slug: 'theme', en: 'Theme', zh: '主题', group: 'configure' },
  { slug: 'resident', en: 'Resident mode', zh: '常驻模式', group: 'run' },
  { slug: 'plugins', en: 'Plugins', zh: '插件', group: 'extend' },
  { slug: 'lua', en: 'Lua plugins', zh: 'Lua 插件', group: 'extend' },
  { slug: 'jsonrpc', en: 'JSON-RPC 2.0', zh: 'JSON-RPC 2.0', group: 'extend' },
]

const pageItem = (page: (typeof pages)[number], prefix: string, lang: 'en' | 'zh') => ({
  text: page[lang],
  link: `${prefix}${page.slug}`,
})

const navItems = (prefix: string, lang: 'en' | 'zh') =>
  pages.filter((page) => page.slug !== 'jsonrpc').map((page) => pageItem(page, prefix, lang))

const nav = (prefix: string, lang: 'en' | 'zh') => [
  { text: lang === 'en' ? 'Guide' : '指南', items: navItems(prefix, lang) },
  { text: 'JSON-RPC', link: `${prefix}jsonrpc` },
]

const sidebar = (prefix: string, lang: 'en' | 'zh') =>
  groups.map((group) => ({
    text: group[lang],
    items: pages
      .filter((page) => page.group === group.key)
      .map((page) => pageItem(page, prefix, lang)),
  }))

export default defineConfig({
  base,
  title: 'WayRun',
  description:
    'A Wayland-native application launcher and quick-search tool for Linux.',
  // Only the English and Chinese guides are site pages; the Chinese README
  // stays a GitHub-facing document.
  srcExclude: ['zh_cn/README_CN.md'],
  head: [['link', { rel: 'icon', type: 'image/svg+xml', href: `${base}logo.svg` }]],

  locales: {
    en: {
      label: 'English',
      lang: 'en',
      link: '/en/',
      themeConfig: {
        nav: nav('/en/', 'en'),
        sidebar: { '/en/': sidebar('/en/', 'en') },
        editLink: {
          pattern: `${repo}/edit/main/docs/:path`,
          text: 'Edit this page on GitHub',
        },
        search: {
          provider: 'local',
          options: {
            translations: {
              button: { buttonText: 'Search', buttonAriaLabel: 'Search' },
              modal: {
                noResultsText: 'No results for',
                resetButtonTitle: 'Clear search',
                footer: {
                  selectText: 'to select',
                  navigateText: 'to navigate',
                  closeText: 'to close',
                },
              },
            },
          },
        },
      },
    },
    zh_cn: {
      label: '简体中文',
      lang: 'zh-CN',
      link: '/zh_cn/',
      themeConfig: {
        nav: nav('/zh_cn/', 'zh'),
        sidebar: { '/zh_cn/': sidebar('/zh_cn/', 'zh') },
        editLink: {
          pattern: `${repo}/edit/main/docs/:path`,
          text: '在 GitHub 上编辑此页',
        },
        outlineTitle: '本页目录',
        docFooter: { prev: '上一页', next: '下一页' },
        darkModeSwitchLabel: '外观',
        lightModeSwitchTitle: '切换到深色模式',
        darkModeSwitchTitle: '切换到浅色模式',
        sidebarMenuLabel: '菜单',
        returnToTopLabel: '回到顶部',
        langMenuLabel: '切换语言',
        skipToContentLabel: '跳到正文',
        footer: {
          message: '以 MIT 或 Apache-2.0 许可发布。',
          copyright: `© ${new Date().getFullYear()} Prslc`,
        },
        search: {
          provider: 'local',
          options: {
            translations: {
              button: { buttonText: '搜索', buttonAriaLabel: '搜索' },
              modal: {
                noResultsText: '没有找到',
                resetButtonTitle: '清除搜索',
                footer: { selectText: '选择', navigateText: '切换', closeText: '关闭' },
              },
            },
          },
        },
      },
    },
  },

  themeConfig: {
    logo: '/logo.svg',
    siteTitle: 'WayRun',
    socialLinks: [{ icon: 'github', link: repo }],
    footer: {
      message: 'Released under the MIT OR Apache-2.0 license.',
      copyright: `© ${new Date().getFullYear()} Prslc`,
    },
  },

  // The demo recording lives above `docs/`.
  vite: {
    server: { fs: { allow: ['..'] } },
  },
})
