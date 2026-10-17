#!/usr/bin/env node
// 构建产物自检：起一个静态服务器（VitePress 的 dist 用绝对路径引用资源，
// 直接 file:// 打开会挂），然后用 playwright 打开文档页检查。
//
// 检查项：页面能开、侧边栏/目录齐全、mermaid 图真的渲染成了 SVG、控制台无报错。

import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { resolve, join, normalize, extname } from 'node:path';
import { chromium } from 'playwright-core';

const CHROME = process.env.CHROME_PATH
  || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const ROOT = resolve('docs/.vitepress/dist');
// 站点带 base，页面 URL 也必须带，否则客户端路由跟 SSR 对不上（hydration mismatch）
const BASE = process.env.SITE_BASE || '/Q2Q/';
// 站点现在分语言、分节，所以自检也不再看单页。默认覆盖英文各节、中文占位页
// 和中文设计文档；也可以显式传页面路径覆盖（npm run e2e -- /Q2Q/guide/）。
const PAGES = process.argv.slice(2).length
  ? process.argv.slice(2)
  : [
      BASE,
      BASE + 'guide/what-is-q2q.html',
      BASE + 'guide/deploy.html',
      BASE + 'guide/transport.html',
      BASE + 'zh/',
      BASE + 'zh/spec.html',
    ];
// 首页是 layout:home，中文指南页目前是占位——两者都没有 h2/h3 锚点
const NO_ANCHORS = [BASE, BASE + 'zh/'];

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript',
  '.css': 'text/css',
  '.svg': 'image/svg+xml',
  '.json': 'application/json',
  '.woff2': 'font/woff2',
};

if (!existsSync(ROOT)) {
  console.error(`${ROOT} 不存在，先跑 npm run build`);
  process.exit(1);
}
if (!existsSync(CHROME)) {
  console.error(`找不到 Chrome：${CHROME}`);
  process.exit(1);
}

// 资源链接同样带 base 前缀，而服务器是拿 dist 当根的，请求进来先剥掉前缀
const server = createServer(async (req, res) => {
  let pathname = decodeURIComponent(req.url.split('?')[0]);
  if (pathname.startsWith(BASE)) pathname = '/' + pathname.slice(BASE.length);
  if (pathname.endsWith('/')) pathname += 'index.html';
  try {
    const data = await readFile(join(ROOT, normalize(pathname)));
    res.writeHead(200, { 'Content-Type': MIME[extname(pathname)] || 'application/octet-stream' });
    res.end(data);
  } catch {
    res.writeHead(404).end('not found');
  }
});

await new Promise(r => server.listen(0, '127.0.0.1', r));
const base = `http://127.0.0.1:${server.address().port}`;

const browser = await chromium.launch({ executablePath: CHROME });
// 站点图标之类的缺失不算问题，404 统计里滤掉
const noise = /favicon|apple-touch-icon/i;

let failed = 0;

for (const PAGE of PAGES) {
  const page = await browser.newPage();
  const errors = [];
  const notFound = [];

  page.on('pageerror', e => errors.push(e.message));
  page.on('console', m => {
    if (m.type() !== 'error') return;
    // 404 单独由 response 统计，这里跳过避免同一件事算两次
    if (/status of 404/.test(m.text())) return;
    errors.push(m.text());
  });
  page.on('response', r => {
    if (r.status() === 404 && !noise.test(r.url())) notFound.push(r.url());
  });

  await page.goto(base + PAGE, { waitUntil: 'load' });

  // 等所有 mermaid 块都渲染完。不能用 waitForSelector——它只等第一个匹配的元素，
  // 会在其余图还在渲染时就返回，把计数搞成 1。
  await page
    .waitForFunction(
      () => {
        const blocks = document.querySelectorAll('.mermaid');
        return blocks.length > 0 && [...blocks].every(el => el.querySelector('svg'));
      },
      { timeout: 20000 },
    )
    .catch(() => {});

  const got = await page.evaluate(() => ({
    title: document.title,
    headings: document.querySelectorAll('h2[id], h3[id]').length,
    sideLinks: document.querySelectorAll('.VPSidebar a').length,
    blocks: document.querySelectorAll('.mermaid').length,
    svgs: document.querySelectorAll('.mermaid svg').length,
  }));

  // 首页没有侧边栏，设计文档暂时没有入口也没有——只有指南各节要求它
  const wantsSidebar = /guide/.test(PAGE);
  const wantsAnchors = !NO_ANCHORS.includes(PAGE);
  const results = [
    ['页面标题', got.title || '(空)', !!got.title],
    ...(wantsAnchors ? [['标题锚点', got.headings, got.headings > 0]] : []),
    ...(wantsSidebar ? [['侧边栏链接', got.sideLinks, got.sideLinks > 0]] : []),
    ['mermaid 渲染', `${got.svgs}/${got.blocks}`, got.svgs === got.blocks],
    ['404 资源', notFound.length, notFound.length === 0],
    ['控制台错误', errors.length, errors.length === 0],
  ];

  let bad = 0;
  console.log(`检查 ${PAGE}`);
  for (const [name, value, ok] of results) {
    console.log(`  ${ok ? '\u2713' : '\u2717'} ${name}: ${value}`);
    if (!ok) bad++;
  }
  if (notFound.length) {
    console.log('  404 资源：');
    [...new Set(notFound)].slice(0, 5).forEach(u => console.log('    ' + u));
  }
  if (errors.length) {
    console.log('  错误信息：');
    errors.slice(0, 5).forEach(e => console.log('    ' + e.split('\n')[0]));
  }
  console.log('');

  failed += bad;
  await page.close();
}

await browser.close();
server.close();

console.log(failed ? `${failed} 项未通过` : '全部通过');
process.exit(failed ? 1 : 0);
