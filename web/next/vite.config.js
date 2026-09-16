import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// Served by the console server under /next/: the built assets live there too. In development the
// console API is proxied from the server's local port.
export default defineConfig({
  base: '/next/',
  plugins: [react(), tailwindcss()],
  build: { outDir: 'dist', emptyOutDir: true, sourcemap: false },
  server: { port: 5181, strictPort: true, proxy: { '/api': 'http://127.0.0.1:4175' } },
  test: { environment: 'jsdom', include: ['tests/**/*.test.{mjs,jsx}'], css: false },
})
