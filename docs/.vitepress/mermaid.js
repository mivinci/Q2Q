// 把 ```mermaid 代码块原样交给浏览器端的 mermaid 渲染。
//
// 必须用 <pre> 而不是 <div>：Vue 编译模板时会折叠文本节点里的空白
// （whitespace: condense），用 div 的话整段图源码会被压成一行，
// mermaid 解析就报 "Parse error on line 1"。pre 是空白敏感标签，能保住换行。
//
// 渲染由 .vitepress/theme/index.js 触发，mermaid 走动态 import，
// 页面上没有图就不会加载它。

export function mermaidBlock(md) {
  const fence = md.renderer.rules.fence;

  md.renderer.rules.fence = (tokens, idx, options, env, self) => {
    const token = tokens[idx];
    if (token.info.trim() !== 'mermaid') return fence(tokens, idx, options, env, self);

    return `<pre class="mermaid">${md.utils.escapeHtml(token.content)}</pre>\n`;
  };
}
