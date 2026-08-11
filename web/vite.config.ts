import { defineConfig } from 'vite';

export default defineConfig({
  // Relative assets work at a GitHub Pages project path as well as at `/`.
  base: './',
  optimizeDeps: { exclude: ['@benjamin-small/browser-terminal'] },
  server: {
    host: true,
    fs: { allow: ['..'] },
  },
});
