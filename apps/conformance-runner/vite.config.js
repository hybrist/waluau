import { defineConfig } from 'vite';
import { playwright } from '@vitest/browser-playwright';
import { waluauWasmPlugin } from '../playground/vite-plugin-waluau-wasm.js';
import { resolve } from 'node:path';

const appRoot = new URL('.', import.meta.url).pathname;

export default defineConfig({
  plugins: [
    waluauWasmPlugin({
      workspaceRoot: resolve(appRoot, '..', '..'),
      outDir: resolve(appRoot, 'src/waluau-wasm'),
    }),
  ],
  test: {
    include: ['tests/**/*.test.js'],
    browser: {
      enabled: true,
      headless: true,
      provider: playwright(),
      instances: [{ browser: 'chromium' }],
    },
  },
});
