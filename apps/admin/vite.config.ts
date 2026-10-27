import tailwindcss from '@tailwindcss/vite';
import adapter from '@sveltejs/adapter-cloudflare';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';
export default defineConfig({
  ssr: { noExternal: ['bits-ui', '@q2q/ui'] },
  plugins: [tailwindcss(), sveltekit({
    paths: process.env.Q2Q_PUBLIC_ORIGIN ? { origin: new URL(process.env.Q2Q_PUBLIC_ORIGIN).origin } : {},
    csrf: { trustedOrigins: ['https://admin.q2q.app'] },
    compilerOptions: { runes: ({ filename }) => filename.split(/[/\\]/).includes('node_modules') ? undefined : true },
    adapter: adapter()
  })]
});
