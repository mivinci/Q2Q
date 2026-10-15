import DefaultTheme from 'vitepress/theme';
import './style.css';
import { onMounted, watch, nextTick } from 'vue';
import { useRoute } from 'vitepress';

// mermaid 只有在页面上真有图时才加载，避免给没有图的页面塞进一个大包
let mermaidPromise = null;

async function renderDiagrams() {
  await nextTick();

  // 只挑还没渲染过的。路由切换会再调一次本函数，
  // 把已经变成 SVG 的元素再丢给 mermaid 会报 Parse error
  const nodes = [...document.querySelectorAll('.mermaid')].filter(el => !el.dataset.processed);
  if (!nodes.length) return;

  const mermaid = await (mermaidPromise ??= import('mermaid').then(m => m.default));

  mermaid.initialize({
    startOnLoad: false,
    securityLevel: 'loose',
    theme: document.documentElement.classList.contains('dark') ? 'dark' : 'default',
  });

  await mermaid.run({ nodes });
}

export default {
  extends: DefaultTheme,
  setup() {
    const route = useRoute();

    onMounted(renderDiagrams);
    watch(() => route.path, renderDiagrams);
  },
};
