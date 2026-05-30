import { defineConfig, devices } from '@playwright/test';
import { existsSync } from 'node:fs';

// In cloud execution environments (Claude Code on the web), Playwright browsers
// are pre-installed at /opt/pw-browsers. Auto-detect this path so that
// `pnpm test` works without extra setup. On developer machines the browsers are
// installed in the default cache location after running
// `pnpm exec playwright install chromium`.
if (!process.env.PLAYWRIGHT_BROWSERS_PATH && existsSync('/opt/pw-browsers')) {
  process.env.PLAYWRIGHT_BROWSERS_PATH = '/opt/pw-browsers';
}

export default defineConfig({
  testDir: './tests',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: 'list',
  use: {
    baseURL: 'http://localhost:4173',
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
  },
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } },
  ],
  // Assumes `pnpm build` has already been run.  Preview serves the built dist
  // without rebuilding Rust/WASM, so startup is fast.
  webServer: {
    command: 'pnpm preview',
    port: 4173,
    reuseExistingServer: !process.env.CI,
    timeout: 10_000,
  },
});
