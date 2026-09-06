import { defineConfig } from 'vite';
import { playwright } from '@vitest/browser-playwright';
import { waluau } from '@waluau/vite-plugin';

export default defineConfig({
  plugins: [waluau({
    manifest: 'waluau.assets.json',
  })],
  test: {
    include: ['src/**/*.test.walu'],
    // This fixed-seed simulation is an economy calibration job, not a unit
    // test, and can legitimately run past Vitest's per-test timeout.
    exclude: ['src/economy.test.walu'],
    browser: {
      enabled: true,
      headless: true,
      provider: playwright(),
      instances: [{ browser: 'chromium' }],
    },
  },
});
