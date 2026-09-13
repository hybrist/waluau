import { defineConfig } from '@playwright/test';
import getPort from 'get-port';

const port = process.env.ANTE_VISUAL_PORT ||= String(await getPort());
export default defineConfig({
  testDir: './visual',
  outputDir: './visual-results',
  snapshotPathTemplate: '{testDir}/baselines/{arg}{ext}',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  forbidOnly: !!process.env.CI,
  reporter: [['list'], ['html', { outputFolder: 'visual-report', open: 'never' }]],
  expect: { toHaveScreenshot: { threshold: 0, maxDiffPixels: 0 } },
  use: {
    browserName: 'chromium',
    viewport: { width: 1200, height: 800 },
    deviceScaleFactor: 1,
    locale: 'en-US',
    timezoneId: 'UTC',
    colorScheme: 'dark',
    reducedMotion: 'no-preference',
    launchOptions: { args: ['--use-angle=swiftshader', '--enable-unsafe-swiftshader'] },
    connectOptions: process.env.ANTE_VISUAL_WS ? { wsEndpoint: process.env.ANTE_VISUAL_WS } : undefined,
    baseURL: `http://${process.env.ANTE_VISUAL_HOST || "127.0.0.1"}:${port}`,
    trace: 'retain-on-failure',
  },
  webServer: {
    env: { __VITE_ADDITIONAL_SERVER_ALLOWED_HOSTS: process.env.ANTE_VISUAL_HOST || "127.0.0.1" },
    command: `${process.env.PLAYWRIGHT_SKIP_BUILD ? '' : 'pnpm build-storybook && '}pnpm exec vite preview --outDir dist/storybook --host 0.0.0.0 --port ${port} --strictPort`,
    url: `http://127.0.0.1:${port}/iframe.html`,
    timeout: 120_000,
  },
});
