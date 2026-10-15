#!/usr/bin/env node
// 把一篇设计文档渲染成一个静态页面，输出到 site/。
//
// mermaid 在构建期用 mermaid-cli 渲染成 SVG，因此产物不依赖任何前端 JS：
// 打开即完整显示，不需要 fetch 原文、不需要加载 marked，也不需要 mermaid 运行时。

import { readFileSync, writeFileSync, mkdirSync, existsSync, rmSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { marked } from 'marked';

const SRC = process.argv[2] || 'docs/design/p2p-technical.md';
const OUT = 'site';
const PUPPETEER_CONFIG = 'scripts/puppeteer.json';
const MMDC_BIN = 'node_modules/.bin/mmdc';
const MMDC_CLI = 'node_modules/@mermaid-js/mermaid-cli/src/cli.js';

const read = f => (existsSync(f) ? readFileSync(f, 'utf8') : '');
const esc = s => s.replace(/[&<>]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[c]));
const stripTags = s => s.replace(/<[^>]+>/g, '');

mkdirSync(OUT, { recursive: true });

// --- 1. mermaid 代码块 -> SVG（渲染失败则退回代码块，不让整篇构建挂掉）---

const source = readFileSync(SRC, 'utf8');
const diagrams = [];
const stripped = source.replace(/```mermaid\r?\n([\s\S]*?)```/g, (_m, code) => {
  diagrams.push(code);
  return `\n@@DIAGRAM-${diagrams.length - 1}@@\n`;
});

console.log(`发现 ${diagrams.length} 个 mermaid 图`);

// 直接看本地二进制在不在。不要用 npx 探测——依赖没装时它会去联网拉 chromium，几百 MB
// 优先用 npm 链接出来的命令；链接失败时（受限环境）直接跑包里的入口
const mmdc = process.env.SKIP_MERMAID === '1'
  ? null
  : existsSync(MMDC_BIN)
    ? [MMDC_BIN]
    : existsSync(MMDC_CLI) ? [process.execPath, MMDC_CLI] : null;
if (!mmdc) console.warn('未找到 mermaid-cli，所有图保留为代码块（先 npm install）');

// MERMAID_RENDERER=ink 时改用 mermaid.ink 在线渲染（无需本地浏览器）。
// 注意：这会把图的内容发到第三方服务，仅用于本地预览，CI 里不要开。
const useInk = process.env.MERMAID_RENDERER === 'ink';
if (useInk && !mmdc) console.log('使用 mermaid.ink 在线渲染（图内容会发往第三方）');

// MERMAID_RENDERER=reuse：直接复用 site/ 里已有的 SVG（由 e2e/render-mermaid.mjs 生成）
// 默认复用 site/ 里已有的 SVG（由 npm run render:mermaid 生成）；
// MERMAID_RENDERER=mmdc 时才改用 mermaid-cli 现场渲染
const reuse = process.env.MERMAID_RENDERER !== 'mmdc';

const svgPaths = [];
for (let i = 0; i < diagrams.length; i++) {
  const svg = `${OUT}/diagram-${i}.svg`;
  let ok = false;

  if (reuse && existsSync(svg)) {
    ok = true;
  } else if (mmdc) {
    const mmd = `${OUT}/.diagram-${i}.mmd`;
    writeFileSync(mmd, diagrams[i]);
    try {
      execFileSync(mmdc[0], [...mmdc.slice(1), '-i', mmd, '-o', svg, '-p', PUPPETEER_CONFIG], { stdio: 'pipe' });
      ok = existsSync(svg);
    } catch (e) {
      console.warn(`  图 ${i + 1} 渲染失败，保留为代码块：${String(e.message).split('\n')[0]}`);
    } finally {
      rmSync(mmd, { force: true });
    }
  } else if (useInk) {
    try {
      const payload = JSON.stringify({ code: diagrams[i], mermaid: { theme: 'default' } });
      const r = await fetch(`https://mermaid.ink/svg/${Buffer.from(payload).toString('base64url')}`);
      if (r.ok) {
        writeFileSync(svg, await r.text());
        ok = true;
      } else {
        console.warn(`  图 ${i + 1} 在线渲染返回 ${r.status}`);
      }
    } catch (e) {
      console.warn(`  图 ${i + 1} 在线渲染失败：${String(e.message).split('\n')[0]}`);
    }
  }

  svgPaths.push(ok ? svg : null);
}

const md = stripped.replace(/@@DIAGRAM-(\d+)@@/g, (_m, iStr) => {
  const i = Number(iStr);
  return svgPaths[i]
    ? `![图 ${i + 1}](diagram-${i}.svg)`
    : `<div class="mermaid-pending"><p>图 ${i + 1}（未渲染：此处在 CI 中会是 SVG）</p><pre><code>${esc(diagrams[i])}</code></pre></div>`;
});

// --- 2. markdown -> HTML，顺带收集标题做目录 ---

const headings = [];
let seq = 0;

marked.use({
  gfm: true,
  renderer: {
    // marked v12 的签名是 (text, level, raw)；改成 token 对象是 v15 的事。
    // package.json 锁在 ^12，别照搬新版的写法。
    heading(text, level) {
      const id = 'sec-' + seq++;
      headings.push({ level, text: stripTags(text), id });
      return `<h${level} id="${id}">${text}</h${level}>\n`;
    },
  },
});

const body = marked.parse(md);
const title = stripTags((source.match(/^#\s+(.+)$/m) || [, '文档'])[1]);

const toc = headings
  .filter(h => h.level <= 3)
  .map(h => `<a class="lv${h.level}" href="#${h.id}">${esc(h.text)}</a>`)
  .join('\n');

// --- 3. 组装单文件页面 ---

const cssLight = read('node_modules/github-markdown-css/github-markdown-light.css');
const cssDark = read('node_modules/github-markdown-css/github-markdown-dark.css');
if (!cssLight) console.warn('未找到 github-markdown-css，页面将只有浏览器默认样式');

const layout = `
  :root { --bg:#fff; --side:#f6f8fa; --border:#d0d7de; --muted:#59636e; --hl:#0969da; }
  @media (prefers-color-scheme: dark) {
    :root { --bg:#0d1117; --side:#161b22; --border:#30363d; --muted:#8b949e; --hl:#58a6ff; }
  }
  * { box-sizing: border-box; }
  html { scroll-behavior: smooth; }
  body { margin:0; background:var(--bg); display:flex; align-items:flex-start;
         font-family:-apple-system,BlinkMacSystemFont,"Segoe UI","PingFang SC","Microsoft YaHei",sans-serif; }
  #toc { position:sticky; top:0; align-self:flex-start; flex:0 0 280px; width:280px;
         height:100vh; overflow-y:auto; padding:24px 12px;
         border-right:1px solid var(--border); background:var(--side); font-size:13px; line-height:1.6; }
  #toc .title { font-size:14px; font-weight:600; color:var(--muted); padding:0 8px 12px; }
  #toc a { display:block; padding:4px 8px; border-radius:6px; color:var(--muted); text-decoration:none; }
  #toc a:hover { background:rgba(125,125,125,.14); }
  #toc a.lv3 { padding-left:22px; font-size:12px; }
  #toc a.active { color:var(--hl); font-weight:600; }
  main { flex:1 1 auto; min-width:0; padding:32px 40px 120px; }
  .markdown-body { max-width:960px; margin:0 auto; background:transparent !important; }
  .markdown-body img { max-width:100%; }
  .markdown-body p > img[src$=".svg"] { display:block; margin:20px auto; background:var(--bg); }
  .mermaid-pending { border:1px dashed var(--border); border-radius:8px; padding:12px 16px; margin:20px 0; }
  .mermaid-pending p { margin:0 0 8px; font-size:12px; color:var(--muted); }
  .mermaid-pending pre { margin:0; }
  @media (max-width:900px) { #toc { display:none; } main { padding:24px 16px 80px; } }
`;

const scrollspy = `
  const links = [...document.querySelectorAll('#toc a')];
  const targets = links.map(a => document.getElementById(a.hash.slice(1)));
  const io = new IntersectionObserver(entries => {
    for (const e of entries) if (e.isIntersecting) {
      links.forEach(a => a.classList.remove('active'));
      const i = targets.indexOf(e.target);
      if (i >= 0) links[i].classList.add('active');
    }
  }, { rootMargin: '-80px 0px -70% 0px' });
  targets.forEach(t => t && io.observe(t));
`;

const html = `<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>${esc(title)}</title>
<style>
@media (prefers-color-scheme: light) { ${cssLight} }
@media (prefers-color-scheme: dark) { ${cssDark} }
${layout}
</style>
</head>
<body>
<nav id="toc"><div class="title">${esc(title)}</div>
${toc}
</nav>
<main class="markdown-body">${body}</main>
<script>${scrollspy}</script>
</body>
</html>
`;

writeFileSync(`${OUT}/index.html`, html);
writeFileSync(`${OUT}/.nojekyll`, '');

const okCount = svgPaths.filter(Boolean).length;
console.log(`写出 ${OUT}/index.html（${headings.length} 个标题，${okCount}/${diagrams.length} 张图已静态化）`);
