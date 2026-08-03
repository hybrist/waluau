import { test, expect } from '@playwright/test';

const COMPILER_READY_TIMEOUT = 20_000;

test('complex project analysis runs in a dedicated worker without starving the UI', async ({ page }) => {
  await page.addInitScript(() => {
    const BrowserWorker = window.Worker;
    window.__compilerWorkers = [];
    window.Worker = class TrackingWorker extends BrowserWorker {
      constructor(url, options) {
        window.__compilerWorkers.push({ url: String(url), type: options?.type ?? 'classic' });
        super(url, options);
      }
    };
  });
  await page.goto('/');
  await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
    timeout: COMPILER_READY_TIMEOUT,
  });

  await page.evaluate(() => {
    const interval = 20;
    let previous = performance.now();
    window.__compilerResponsiveness = { samples: 0, maxDelay: 0 };
    window.__compilerResponsivenessTimer = setInterval(() => {
      const now = performance.now();
      window.__compilerResponsiveness.samples += 1;
      window.__compilerResponsiveness.maxDelay = Math.max(
        window.__compilerResponsiveness.maxDelay,
        now - previous - interval,
      );
      previous = now;
    }, interval);
  });

  await page.getByRole('button', { name: 'Ante Magic' }).click();
  await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
    timeout: COMPILER_READY_TIMEOUT,
  });

  const observed = await page.evaluate(() => {
    clearInterval(window.__compilerResponsivenessTimer);
    return {
      workers: window.__compilerWorkers,
      responsiveness: window.__compilerResponsiveness,
    };
  });
  const compilerWorkers = observed.workers.filter(({ url }) =>
    url.includes('waluauCompiler.worker')
  );
  expect(compilerWorkers).toHaveLength(1);
  expect(compilerWorkers[0].type).toBe('module');
  expect(observed.responsiveness.samples).toBeGreaterThan(1);
  expect(observed.responsiveness.maxDelay).toBeLessThan(1_000);
});

test('a superseded analysis cannot replace the newest editor result', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
    timeout: COMPILER_READY_TIMEOUT,
  });

  const editor = page.locator('.code-textarea');
  await editor.fill(`${'local value: i32 = 1\n'.repeat(2_000)}???`);
  await editor.fill('function newest(): i32\n    return 42\nend');

  await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded', {
    timeout: COMPILER_READY_TIMEOUT,
  });
  await page.waitForTimeout(250);
  await expect(page.locator('.status-text')).toHaveText('Compilation Succeeded');
  await page.getByRole('button', { name: 'Generated IR' }).click();
  await expect(page.locator('.ir-output')).toContainText('newest');
});
