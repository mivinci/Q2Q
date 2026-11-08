import { fileURLToPath } from 'node:url';
import tailwindcss from '@tailwindcss/vite';
import adapter from '@sveltejs/adapter-cloudflare';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';
export default defineConfig({
  server: { fs: { allow: ['.', '../../packages/ui', '../../packages/client-web', '../../crates/q2q-wasm/pkg', '../../node_modules'].map(path => fileURLToPath(new URL(path, import.meta.url))) } },
  ssr: { noExternal: ['bits-ui', '@q2q/ui', '@q2q/client-web'] },
  plugins: [tailwindcss(), sveltekit({
    compilerOptions: { runes: ({ filename }) => filename.split(/[/\\]/).includes('node_modules') ? undefined : true },
    adapter: adapter()
  })]
});
