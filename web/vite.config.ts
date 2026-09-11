import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import tailwindcss from '@tailwindcss/vite'
import { resolve } from 'node:path'

// The SPA is embedded into the Rust binary and served from the site root, so assets are
// emitted with absolute `/assets/...` paths (the default base "/"). Output goes to web/dist,
// which rust-embed bakes in at compile time.
export default defineConfig({
  plugins: [vue(), tailwindcss()],
  resolve: { alias: { '@': resolve(__dirname, 'src') } },
  build: { outDir: 'dist', emptyOutDir: true },
  server: {
    port: 5173,
    // `make dev` runs vite here and the Rust server on 8080; everything the SPA calls is
    // proxied so the dev origin matches the embedded one.
    proxy: {
      '/api': 'http://localhost:8080',
      '/mcp': 'http://localhost:8080',
      '/oauth': 'http://localhost:8080',
      '/login': 'http://localhost:8080',
    },
  },
})
