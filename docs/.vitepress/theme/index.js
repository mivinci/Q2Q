import DefaultTheme from 'vitepress/theme';
import './style.css';
import { onMounted, watch, nextTick } from 'vue';
import { useRoute, useData } from 'vitepress';
import { diagramConfig } from './mermaid-theme.js';

// Load Mermaid only on pages with diagrams. Keep the source outside the DOM so
// theme changes never try to parse a previously rendered SVG as Mermaid syntax.
let mermaidPromise = null;
const sources = new WeakMap();
let queue = Promise.resolve();
let revision = 0;
let diagramId = 0;

function renderDiagrams(dark) {
  const requested = ++revision;
  const theme = dark ? 'dark' : 'light';
  // Mermaid's configuration is global. Serialize renders and skip obsolete work
  // when a route or appearance change happens during its asynchronous import/render.
  queue = queue.then(async () => {
    await nextTick();
    if (requested !== revision) return;
    const nodes = [...document.querySelectorAll('.mermaid')]
      .filter(el => el.dataset.theme !== theme);
    if (!nodes.length) return;

    const mermaid = await (mermaidPromise ??= import('mermaid').then(m => m.default));
    if (requested !== revision) return;
    mermaid.initialize(diagramConfig(dark));

    for (const node of nodes) {
      if (requested !== revision) return;
      if (!node.isConnected) continue;
      if (!sources.has(node)) sources.set(node, node.textContent);
      const { svg, bindFunctions } = await mermaid.render(`q2q-diagram-${++diagramId}`, sources.get(node));
      if (requested !== revision) return;
      if (!node.isConnected) continue;
      node.innerHTML = svg;
      node.dataset.processed = 'true';
      node.dataset.theme = theme;
      bindFunctions?.(node);
    }
  }).catch(error => console.error('Mermaid rendering failed:', error));
  return queue;
}

export default {
  extends: DefaultTheme,
  setup() {
    const route = useRoute();
    const { isDark } = useData();

    onMounted(() => renderDiagrams(isDark.value));
    watch([() => route.path, isDark], () => renderDiagrams(isDark.value));

    // Wait for initial navbar classes before restoring its transitions.
    onMounted(() => nextTick(() => document.documentElement.classList.add('ready')));
  },
};
