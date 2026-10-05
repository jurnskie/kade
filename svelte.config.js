// Tauri doesn't have a Node.js server to do proper SSR
// so we use adapter-static with a fallback to index.html to put the site in SPA mode
// See: https://svelte.dev/docs/kit/single-page-apps
// See: https://v2.tauri.app/start/frontend/sveltekit/ for more info
import adapter from "@sveltejs/adapter-static";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess(),
  // Ship component CSS inside the JS. Vite's separate virtual CSS modules can
  // fail to load on a cold dev start, leaving the app unstyled; a desktop app
  // without SSR loses nothing by injecting them.
  compilerOptions: { css: "injected" },
  kit: {
    adapter: adapter({
      fallback: "index.html",
    }),
  },
};

export default config;
