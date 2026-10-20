#!/usr/bin/env node
// 构建产物自检：起一个静态服务器（VitePress 的 dist 用绝对路径引用资源，
// 直接 file:// 打开会挂），然后用 playwright 打开文档页检查。
//
// 检查页面、侧边栏/目录、Mermaid 渲染与明暗切换、多行标签和窄屏滚动；
// 最后覆盖客户端页面导航及连续主题切换，确认异步重绘不会覆盖新页面。

import { existsSync } from 'node:fs';
import { mkdir, readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { resolve, join, normalize, extname } from 'node:path';
import { chromium } from 'playwright-core';

const CHROME = process.env.CHROME_PATH
  || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const ROOT = resolve('docs/.vitepress/dist');
// 独立文档域名部署在站点根路径；保留 SITE_BASE 以便本地验证子路径部署。
const BASE = process.env.SITE_BASE || '/';
// 站点现在分语言、分节，所以自检也不再看单页。默认覆盖英文首页与指南、
// 中文首页、关键中文指南和中文设计文档；也可以显式传页面路径覆盖。
const PAGES = process.argv.slice(2).length
  ? process.argv.slice(2)
  : [
      BASE,
      BASE + 'guide/what-is-q2q.html',
      BASE + 'guide/deploy.html',
      BASE + 'guide/transport.html',
      BASE + 'zh/',
      BASE + 'zh/guide/what-is-q2q.html',
      BASE + 'zh/guide/deploy.html',
      BASE + 'zh/guide/punch.html',
      BASE + 'zh/spec.html',
    ];
// 首页是 layout:home，没有 h2/h3 锚点
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
  const page = await browser.newPage({ colorScheme: 'light' });
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
        return [...blocks].every(el => el.querySelector('svg'));
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

  const appearances = [];
  if (got.blocks && got.svgs === got.blocks) {
    let previousFill;
    const labelsFit = await page.evaluate(() =>
      [...document.querySelectorAll('.mermaid foreignObject')].every(el => {
        const p = el.querySelector('p');
        return !p || p.getBoundingClientRect().height <= el.getBoundingClientRect().height + 1;
      }),
    );
    appearances.push(['多行标签完整显示', labelsFit, labelsFit]);
    let previousSvg = await page.locator('.mermaid svg').first().getAttribute('id');
    for (const theme of ['dark', 'light']) {
      await page.emulateMedia({ colorScheme: theme });
      let changed = true;
      await page.waitForFunction(
        ({ theme, previousSvg, count }) => {
          const blocks = [...document.querySelectorAll('.mermaid')];
          return blocks.length === count
            && blocks.every(el => el.dataset.theme === theme && el.querySelector('svg'))
            && blocks[0].querySelector('svg').id !== previousSvg;
        },
        { theme, previousSvg, count: got.blocks },
        { timeout: 20000 },
      ).catch(() => { changed = false; });
      const svg = page.locator('.mermaid svg').first();
      const currentSvg = await svg.getAttribute('id');
      const fill = await page.evaluate(() => {
        const shape = document.querySelector('.mermaid .node rect, .mermaid rect.actor');
        return shape ? getComputedStyle(shape).fill : null;
      });
      appearances.push([`${theme} 主题重绘`, fill || '(无矩形节点)', changed]);
      if (theme === 'light' && previousFill && fill) {
        appearances.push(['节点配色随主题切换', `${previousFill} → ${fill}`, previousFill !== fill]);
      }
      previousFill = fill;
      previousSvg = currentSvg;
      if (process.env.MERMAID_SCREENSHOTS) {
        await mkdir(process.env.MERMAID_SCREENSHOTS, { recursive: true });
        const name = PAGE.replace(/[^a-zA-Z0-9-]/g, '-');
        await page.locator('.mermaid').first().screenshot({
          path: join(process.env.MERMAID_SCREENSHOTS, `${name}-${theme}.png`),
        });
      }
    }
  }

  if (got.blocks && got.svgs === got.blocks) {
    await page.setViewportSize({ width: 390, height: 844 });
    const contained = await page.evaluate(() =>
      [...document.querySelectorAll('.mermaid')].every(el => {
        const bounds = el.getBoundingClientRect();
        const svg = el.querySelector('svg');
        return bounds.left >= 0 && bounds.right <= innerWidth + 1
          && svg.getBoundingClientRect().width + 1 >= svg.viewBox.baseVal.width;
      }),
    );
    appearances.push(['窄屏图框与文字尺寸', contained, contained]);
  }

  // 首页没有侧边栏，设计文档暂时没有入口也没有——只有指南各节要求它
  const wantsSidebar = /guide/.test(PAGE);
  const wantsAnchors = !NO_ANCHORS.includes(PAGE);
  const results = [
    ['页面标题', got.title || '(空)', !!got.title],
    ...(wantsAnchors ? [['标题锚点', got.headings, got.headings > 0]] : []),
    ...(wantsSidebar ? [['侧边栏链接', got.sideLinks, got.sideLinks > 0]] : []),
    ['mermaid 渲染', `${got.svgs}/${got.blocks}`, got.svgs === got.blocks],
    ...appearances,
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

// Exercise in-app navigation while renders/theme changes can still be queued.
const navigation = await browser.newPage({ colorScheme: 'light' });
const navigationErrors = [];
navigation.on('pageerror', error => navigationErrors.push(error.message));
navigation.on('console', message => {
  if (message.type() === 'error') navigationErrors.push(message.text());
});
try {
  await navigation.goto(base + BASE + 'guide/deploy.html');
  await navigation.waitForFunction(() => document.querySelector('.mermaid[data-theme="light"] svg'));
  await navigation.emulateMedia({ colorScheme: 'dark' });
  await navigation.locator(`.VPSidebar a[href="${BASE}guide/transport"]`).first().click();
  await navigation.emulateMedia({ colorScheme: 'light' });
  await navigation.emulateMedia({ colorScheme: 'dark' });
  await navigation.waitForFunction(() => {
    const blocks = [...document.querySelectorAll('.mermaid')];
    return location.pathname.endsWith('/guide/transport') && blocks.length === 1
      && blocks.every(el => el.dataset.theme === 'dark' && el.querySelector('svg .node'));
  }, null, { timeout: 20000 });
  await navigation.locator(`.VPSidebar a[href="${BASE}guide/deploy"]`).first().click();
  await navigation.emulateMedia({ colorScheme: 'light' });
  await navigation.waitForFunction(() => {
    const blocks = [...document.querySelectorAll('.mermaid')];
    return blocks.length === 2 && blocks.every(el => el.dataset.theme === 'light' && el.querySelector('svg'));
  }, null, { timeout: 20000 });
  if (navigationErrors.length) throw new Error(navigationErrors.join('\n'));
  console.log('✓ 页面切换及连续主题切换后图形正确，无控制台错误');
} catch (error) {
  failed++;
  console.log(`✗ 页面/主题切换回归: ${error.message}`);
} finally {
  await navigation.close();
}

await browser.close();
server.close();

console.log(failed ? `${failed} 项未通过` : '全部通过');
process.exit(failed ? 1 : 0);
