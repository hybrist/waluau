import { test, expect } from '@playwright/test';
import { installScryingProbe } from './scrying-probe.js';

async function openCity(page) {
  await installScryingProbe(page);
  await page.goto('/');
  await expect.poll(() => page.evaluate(() => window.scryingProbe.captures.length), { timeout: 20_000 }).toBeGreaterThan(2);
}

test('live lens follows reduced motion, including preference changes', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await openCity(page);
  expect(await page.evaluate(() => [...new Set(window.scryingProbe.clocks)])).toEqual([0]);
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await expect.poll(() => page.evaluate(() => new Set(window.scryingProbe.clocks).size)).toBeGreaterThan(2);
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.evaluate(() => { window.scryingProbe.clocks = []; });
  await expect.poll(() => page.evaluate(() => window.scryingProbe.clocks.length)).toBeGreaterThan(4);
  expect(await page.evaluate(() => [...new Set(window.scryingProbe.clocks)])).toEqual([0]);
});

test('city and route captures match dense displays with a two-times cap', async ({ browser, baseURL }) => {
  const sizes = [];
  for (const deviceScaleFactor of [1, 2, 3]) {
    const context = await browser.newContext({ baseURL, deviceScaleFactor, viewport: { width: 960, height: 640 } });
    try {
      const page = await context.newPage();
      await openCity(page);
      const captures = await page.evaluate(() => window.scryingProbe.captures.slice(-2));
      expect(captures[0]).toEqual(captures[1]);
      sizes.push(captures[0]);
    } finally {
      await context.close();
    }
  }
  expect(sizes[1][0]).toBeGreaterThan(sizes[0][0]);
  expect(sizes[1][1]).toBeGreaterThan(sizes[0][1]);
  expect(sizes[2]).toEqual(sizes[1]);
});
