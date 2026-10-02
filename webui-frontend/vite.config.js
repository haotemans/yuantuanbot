import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

// 构建产物直指 crates/webui/static（rust-embed 内嵌，产物入库以便无 node 的机器也能 cargo build；见根 README）
export default defineConfig({
  plugins: [vue()],
  server: {
    port: 5173,
    proxy: {
      '/api': 'http://127.0.0.1:8085',
      '/healthz': 'http://127.0.0.1:8085',
      '/ws': { target: 'ws://127.0.0.1:8085', ws: true },
    },
  },
  build: {
    outDir: '../crates/webui/static',
    emptyOutDir: false, // 保留 static/legacy-index.html 兜底页
    chunkSizeWarningLimit: 2500,
  },
})
