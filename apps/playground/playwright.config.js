import { defineConfig, devices } from '@playwright/test';
import getPort from 'get-port';
import { existsSync } from 'node:fs';

// In cloud execution environments (Claude Code on the web), Playwright browsers
// are pre-installed at /opt/pw-browsers. Auto-detect this path so that
// `pnpm test` works without extra setup. On developer machines the browsers are
// installed in the default cache location after running
// `pnpm exec playwright install chromium`.
if (!process.env.PLAYWRIGHT_BROWSERS_PATH && existsSync('/opt/pw-browsers')) {
  process.env.PLAYWRIGHT_BROWSERS_PATH = '/opt/pw-browsers';
}

const reuseExistingServer = !!process.env.REUSE_EXISTING_SERVER;
const skipBuild = !!process.env.PLAYWRIGHT_SKIP_BUILD;
const serverPort = reuseExistingServer
  ? 4173
  : process.env.PLAYGROUND_PORT || (await getPort());
const previewCommand = `pnpm preview --host=127.0.0.1 --strictPort --port=${serverPort}`;

export default defineConfig({
  testDir: './tests',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: 'list',
  use: {
    baseURL: `http://127.0.0.1:${serverPort}`,
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  // Build by default so a local test run cannot preview an ignored, stale dist
  // from another checkout state. CI sets PLAYWRIGHT_SKIP_BUILD after its
  // explicit production build to avoid doing the same work twice.
  webServer: {
    command: skipBuild ? previewCommand : `pnpm build && ${previewCommand}`,
    env: {
      ...process.env,
      PLAYWRIGHT_TEST: '1',
      // We need to disable color output to avoid ANSI escape codes for our regex below.
      FORCE_COLOR: undefined,
      NO_COLOR: 'true',
    },
    stdout: 'pipe',
    reuseExistingServer,
    timeout: skipBuild ? 10_000 : 120_000,
    wait: {
      stdout: /Local:\s+http:\/\/127.0.0.1:(?<PLAYGROUND_PORT>\d+)\//,
    },
  },
});
