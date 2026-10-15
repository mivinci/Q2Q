import { defineConfig } from 'vitepress';
import { mermaidBlock } from './mermaid.js';

export default defineConfig({
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

    nav: [
      { text: '设计文档', link: '/spec' },
    ],

    // 右上角图标；icon 除内置名外也支持自定义 svg
    socialLinks: [
      { icon: 'github', link: 'https://github.com/mivinci/Q2Q' },
    ],

    // 每页底部的「编辑此页」链接
    editLink: {
      pattern: 'https://github.com/mivinci/Q2Q/edit/main/docs/:path',
      text: '在 GitHub 上编辑此页',
    },

    sidebar: [
      {
        text: '设计文档',
        items: [{ text: 'Q2Q 网络设计', link: '/spec' }],
      },
    ],

    outline: [2, 3],
    search: { provider: 'local' },

    footer: {
      message: '设计稿，随时会改',
    },
  },
});
