import { defineConfig } from 'vitepress';
import { mermaidBlock } from './mermaid.js';

// English is the root locale; Simplified Chinese lives under /zh/. The design
// document is still Chinese-only, so it sits in the Chinese locale and is not
// linked from either nav until it is translated.
const guide = (prefix) => [
  { text: 'What is Q2Q?', link: `${prefix}guide/what-is-q2q` },
  { text: 'Security', link: `${prefix}guide/security` },
  { text: 'Command-line client', link: `${prefix}guide/cli` },
  { text: 'Track', link: `${prefix}guide/track` },
  { text: 'Punch', link: `${prefix}guide/punch` },
  { text: 'Transport', link: `${prefix}guide/transport` },
  { text: 'Wire protocol', link: `${prefix}guide/wire` },
  { text: 'Scheduling', link: `${prefix}guide/scheduling` },
  { text: 'CDN fallback', link: `${prefix}guide/cdn` },
];

// 侧边栏用数组而不是按路径 keyed，让整站共用一份目录，
// 跟 VitePress 官网一样；首页是 layout:home，本来就不显示侧边栏。
// collapsed: false = 默认展开、带折叠箭头；不写则永远展开、没有箭头。
const sidebar = (prefix, t) => [
  {
    text: t.intro,
    collapsed: false,
    items: [
      { text: t.overview, link: `${prefix}guide/what-is-q2q` },
      { text: t.security, link: `${prefix}guide/security` },
      { text: t.cli, link: `${prefix}guide/cli` },
    ],
  },
  {
    text: t.deploy,
    collapsed: false,
    items: [
      { text: t.singleNode, link: `${prefix}guide/deploy-single-node` },
      { text: t.cluster, link: `${prefix}guide/deploy-cluster` },
    ],
  },
  {
    text: t.planes,
    collapsed: false,
    items: [
      { text: 'Track', link: `${prefix}guide/track` },
      { text: 'Punch', link: `${prefix}guide/punch` },
    ],
  },
  {
    text: t.data,
    collapsed: false,
    items: [
      { text: t.transport, link: `${prefix}guide/transport` },
      { text: t.wire, link: `${prefix}guide/wire` },
      { text: t.sched, link: `${prefix}guide/scheduling` },
      { text: t.cdn, link: `${prefix}guide/cdn` },
    ],
  },
  { text: t.api, link: `${prefix}api/` },
];

const en = {
  intro: 'Introduction',
  overview: 'What is Q2Q?',
  security: 'Security',
  cli: 'Command-line client',
  planes: 'Control Plane',
  deploy: 'Deployment',
  singleNode: 'Single node',
  cluster: 'Cluster',
  data: 'Data Plane',
  transport: 'Transport',
  wire: 'Wire protocol',
  sched: 'Scheduling',
  cdn: 'CDN fallback',
  api: 'API',
};

const zh = {
  intro: '介绍',
  overview: 'Q2Q 是什么？',
  security: '安全机制',
  cli: '命令行客户端',
  planes: '控制面',
  deploy: '部署',
  singleNode: '单机部署',
  cluster: '集群部署',
  data: '数据面',
  transport: '传输层',
  wire: '消息编码',
  sched: '调度',
  cdn: 'CDN 回退',
  api: 'API 参考',
};

export default defineConfig({
  title: 'Q2Q',
  description: 'Q2Q：开源 P2P 网络框架，控制面、数据面与种子格式',
  srcDir: '.',
  outDir: '.vitepress/dist',
  cleanUrls: true,

  // English sources live under en/ but keep the site root as their URL, so
  // English stays the default locale; en/ is a directory, not a locale.
  rewrites: {
    'en/:rest*': ':rest*',
  },

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
            link: '/guide/what-is-q2q',
            activeMatch: '/guide/(?!deploy-)',
            items: guide('/'),
          },
          {
            text: 'Deployment',
            activeMatch: '/guide/deploy-',
            items: [
              { text: 'Single node', link: '/guide/deploy-single-node' },
              { text: 'Cluster', link: '/guide/deploy-cluster' },
            ],
          },
          { text: 'API', link: '/api/' },
        ],

        // 每页底部的「编辑此页」链接
        editLink: {
          pattern: 'https://github.com/mivinci/Q2Q/edit/main/docs/:path',
          text: 'Edit this page on GitHub',
        },

        sidebar: sidebar('/', en),

        footer: {
          copyright: '© 2026 Q2Q · <a href="/privacy">Privacy Policy</a>',
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
            link: '/zh/guide/what-is-q2q',
            activeMatch: '/zh/guide/(?!deploy-)',
            items: [
              { text: 'Q2Q 是什么？', link: '/zh/guide/what-is-q2q' },
              { text: '安全机制', link: '/zh/guide/security' },
              { text: '命令行客户端', link: '/zh/guide/cli' },
              { text: 'Track', link: '/zh/guide/track' },
              { text: 'Punch', link: '/zh/guide/punch' },
              { text: '传输层', link: '/zh/guide/transport' },
              { text: '消息编码', link: '/zh/guide/wire' },
              { text: '调度', link: '/zh/guide/scheduling' },
              { text: 'CDN 回退', link: '/zh/guide/cdn' },
            ],
          },
          {
            text: '部署',
            activeMatch: '/zh/guide/deploy-',
            items: [
              { text: '单机部署', link: '/zh/guide/deploy-single-node' },
              { text: '集群部署', link: '/zh/guide/deploy-cluster' },
            ],
          },
          { text: 'API 参考', link: '/zh/api/' },
        ],

        editLink: {
          pattern: 'https://github.com/mivinci/Q2Q/edit/main/docs/:path',
          text: '在 GitHub 上编辑此页',
        },

        sidebar: sidebar('/zh/', zh),

        footer: {
          copyright: '© 2026 Q2Q · <a href="/zh/privacy">隐私政策</a>',
        },
      },
    },
  },
});
