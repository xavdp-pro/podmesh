import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// Served by the origin under /admin/: the built assets live there too.
export default defineConfig({
  base: '/admin/',
  plugins: [react(), tailwindcss()],
  build: { outDir: 'dist', emptyOutDir: true, sourcemap: false },
  server: { port: 5180, strictPort: true, proxy: { '/admin/api': 'http://127.0.0.1:8080', '/ready': 'http://127.0.0.1:8080' } },
  test: { environment: 'node', include: ['tests/**/*.test.mjs'] },
})
