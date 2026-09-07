import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import { fileURLToPath, URL } from 'node:url'

// 开发代理：后端默认 3220（config.example.yaml server.listen）。可用环境变量覆盖：
//   VITE_PROXY_TARGET=http://127.0.0.1:3220 npm run dev
const target = process.env.VITE_PROXY_TARGET || 'http://127.0.0.1:3220'

export default defineConfig({
  plugins: [vue()],
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },
  // 生产产物为相对路径，便于任意前缀挂载（rust-embed 内嵌在 / 下也可直接用）
  base: './',
  server: {
    port: 5173,
    proxy: {
      '/api': { target, changeOrigin: true },
      '/healthz': { target, changeOrigin: true },
    },
  },
  build: {
    outDir: 'dist',
    chunkSizeWarningLimit: 1500,
  },
})
