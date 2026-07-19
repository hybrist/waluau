import { defineConfig } from 'vite';
import { playwright } from '@vitest/browser-playwright';
import { waluau } from '@waluau/vite-plugin';

export default defineConfig({
  plugins: [waluau({ manifest: 'waluau.assets.json' })],
  test: {
    include: ['src/**/*.test.walu'],
    browser: {
      enabled: true,
      headless: true,
      provider: playwright(),
      instances: [{ browser: 'chromium' }],
    },
  },
});
