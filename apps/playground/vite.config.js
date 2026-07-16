import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

import { waluauWasmPlugin } from './vite-plugin-waluau-wasm.js'

const rootDir = fileURLToPath(new URL('.', import.meta.url))
const workspaceRoot = resolve(rootDir, '../..')

// https://vite.dev/config/
export default defineConfig(({ command }) => ({
  plugins: [
    waluauWasmPlugin({
      workspaceRoot,
      production: command === 'build',
    }),
    react(),
  ],
  build: {
    rollupOptions: {
      input: {
        // The playground itself, plus one standalone page per example. Each
        // `output/<example>.html` is served at `/output/<example>`.
        main: resolve(rootDir, 'index.html'),
        'output/poker-tricks': resolve(rootDir, 'output/poker-tricks.html'),
      },
    },
  },
  server: {
    fs: {
      allow: [workspaceRoot],
    },
  },
}))
