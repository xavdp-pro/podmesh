import { defineConfig, loadEnv } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

/**
 * Vite dev server: binds loopback only and proxies /api and /socket.io to the
 * API. The public reverse proxy (Cloudflare tunnel, nginx) targets VITE_PORT in
 * development; in production it targets PORT with SERVE_DIST=true instead.
 */
export default defineConfig(({ mode }) => {
  const env = { ...loadEnv(mode, process.cwd(), ''), ...process.env }
  const vitePort = Number(env.VITE_PORT || 7701)
  const apiPort = Number(env.PORT || 7700)
  const hmrHost = env.VITE_HMR_HOST || ''
  const allowedHosts = (env.VITE_ALLOWED_HOSTS || '').split(',').map((s) => s.trim()).filter(Boolean)

  return {
    plugins: [react(), tailwindcss()],
    resolve: { dedupe: ['react', 'react-dom'] },
    server: {
      host: '127.0.0.1',
      port: vitePort,
      strictPort: true,
      allowedHosts: allowedHosts.length ? allowedHosts : ['localhost', '127.0.0.1'],
      headers: { 'Cache-Control': 'no-store' },
      hmr: hmrHost
        ? { host: hmrHost, protocol: 'wss', clientPort: Number(env.VITE_HMR_CLIENT_PORT || 443), overlay: true }
        : { overlay: true },
      proxy: {
        '/api/': `http://127.0.0.1:${apiPort}`,
        '/socket.io': { target: `http://127.0.0.1:${apiPort}`, ws: true },
      },
    },
    test: {
      environment: 'node',
      include: ['tests/**/*.test.js'],
    },
  }
})
