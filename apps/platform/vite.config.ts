import tailwindcss from '@tailwindcss/vite';
import adapter from '@sveltejs/adapter-cloudflare';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
  // Workspace-hoisted Svelte source packages must be compiled during SSR too.
  ssr: { noExternal: ['bits-ui'] },
	plugins: [
		tailwindcss(),
		sveltekit({
			csrf: { trustedOrigins: ['https://platform.q2q.app'] },
			compilerOptions: {
				// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},
			adapter: adapter()
		})
	]
});
