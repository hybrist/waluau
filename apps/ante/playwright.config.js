import { defineConfig, devices } from '@playwright/test';
import getPort from 'get-port';
import { existsSync } from 'node:fs';

if (!process.env.PLAYWRIGHT_BROWSERS_PATH && existsSync('/opt/pw-browsers')) {
  process.env.PLAYWRIGHT_BROWSERS_PATH = '/opt/pw-browsers';
}

const skipBuild = !!process.env.PLAYWRIGHT_SKIP_BUILD;
const serverPort = process.env.ANTE_PORT || (await getPort());
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
  webServer: {
    command: skipBuild ? previewCommand : `pnpm build && ${previewCommand}`,
    env: { ...process.env, NO_COLOR: 'true' },
    stdout: 'pipe',
    timeout: skipBuild ? 10_000 : 120_000,
    wait: {
      stdout: /Local:\s+http:\/\/127\.0\.0\.1:(?<ANTE_PORT>\d+)\//,
    },
  },
});
