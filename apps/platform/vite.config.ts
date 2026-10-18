import tailwindcss from '@tailwindcss/vite';
import adapter from '@sveltejs/adapter-cloudflare';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

// Cloudflare may provide an internal request URL to the Worker runtime. Set
// this only in production builds so SvelteKit's same-origin and CSRF checks
// use the public platform hostname; local preview keeps deriving its own URL.
const publicOrigin = process.env.Q2Q_PUBLIC_ORIGIN;

export default defineConfig({
  // Workspace-hoisted Svelte source packages must be compiled during SSR too.
  ssr: { noExternal: ['bits-ui'] },
	plugins: [
		tailwindcss(),
		sveltekit({
			paths: publicOrigin ? { origin: new URL(publicOrigin).origin } : {},
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
