import { defineConfig } from 'vitepress';
import { mermaidBlock } from './mermaid.js';

// English is the root locale; Simplified Chinese lives under /zh/. The design
// document is still Chinese-only, so it sits in the Chinese locale and is not
// linked from either nav until it is translated.
const guide = (prefix) => [
  { text: 'Overview', link: `${prefix}guide/` },
  { text: 'Deploy', link: `${prefix}guide/deploy` },
  { text: 'Track', link: `${prefix}guide/track` },
  { text: 'Punch', link: `${prefix}guide/punch` },
];

export default defineConfig({
  // 部署在 le0.me/Q2Q/ 子路径下，资源链接要带这个前缀，否则全 404
  base: '/Q2Q/',
  title: 'Q2Q',
  description: 'Q2Q：开源 P2P 网络框架，控制面、数据面与种子格式',
  srcDir: '.',
  outDir: '.vitepress/dist',
  cleanUrls: true,

  markdown: {
    config: md => md.use(mermaidBlock),
  },

  head: [
    ['link', { rel: 'icon', href: '/favicon.svg' }],
  ],

  themeConfig: {
    // 深浅两套：描边跟随主题，不用固定品牌色，免得某一边对比度不够
    logo: {
      light: '/logo-light.svg',
      dark: '/logo-dark.svg',
      alt: 'Q2Q',
    },

    // 右上角图标；icon 除内置名外也支持自定义 svg
    socialLinks: [
      { icon: 'github', link: 'https://github.com/mivinci/Q2Q' },
    ],

    outline: [2, 3],
    search: { provider: 'local' },
  },

  locales: {
    root: {
      label: 'English',
      lang: 'en',
      themeConfig: {
        nav: [
          {
            text: 'Guide',
            link: '/guide/',
            activeMatch: '/guide/',
            items: guide('/'),
          },
          { text: 'API', link: '/api/' },
        ],

        // 每页底部的「编辑此页」链接
        editLink: {
          pattern: 'https://github.com/mivinci/Q2Q/edit/main/docs/:path',
          text: 'Edit this page on GitHub',
        },

        sidebar: {
          '/guide/': [{ text: 'Guide', items: guide('/') }],
          '/api/': [{ text: 'API', items: [{ text: 'Reference', link: '/api/' }] }],
        },

        footer: {
          message: 'Work in progress',
        },
      },
    },

    zh: {
      label: '简体中文',
      lang: 'zh-CN',
      themeConfig: {
        nav: [
          {
            text: '指南',
            link: '/zh/guide/',
            activeMatch: '/zh/guide/',
            items: [
              { text: '总览', link: '/zh/guide/' },
              { text: '部署', link: '/zh/guide/deploy' },
              { text: 'Track', link: '/zh/guide/track' },
              { text: 'Punch', link: '/zh/guide/punch' },
            ],
          },
          { text: 'API 参考', link: '/zh/api/' },
        ],

        editLink: {
          pattern: 'https://github.com/mivinci/Q2Q/edit/main/docs/:path',
          text: '在 GitHub 上编辑此页',
        },

        sidebar: {
          '/zh/guide/': [
            {
              text: '指南',
              items: [
                { text: '总览', link: '/zh/guide/' },
                { text: '部署', link: '/zh/guide/deploy' },
                { text: 'Track', link: '/zh/guide/track' },
                { text: 'Punch', link: '/zh/guide/punch' },
              ],
            },
          ],
          '/zh/api/': [
            { text: 'API 参考', items: [{ text: '概览', link: '/zh/api/' }] },
          ],
        },

        footer: {
          message: '设计稿，随时会改',
        },
      },
    },
  },
});
