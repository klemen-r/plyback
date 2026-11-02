/// <reference types="vitest/config" />
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { defineConfig } from 'vite'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    port: 5173,
    // Keep the browser's Host so the API can enforce same-origin CSRF checks during development.
    proxy: { '/api': { target: process.env.API_URL || 'http://127.0.0.1:8000', changeOrigin: false } },
  },
  build: { outDir: 'dist', chunkSizeWarningLimit: 1200 },
  test: { include: ['src/**/*.test.ts'] },
})
