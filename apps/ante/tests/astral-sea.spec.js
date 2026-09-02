import { test, expect } from '@playwright/test';
import {
  GAME_READY_TIMEOUT,
  beginHeist,
  countCardBackInk,
  openGame,
} from './game-driver.js';

// Sample the slim margins outside Ante's packed 700-unit board. They contain
// the backdrop but no cards, controls, or labels, so changes here come from the
// astral sea itself rather than the opening deal.
function astralMarginMetrics(canvas) {
  return canvas.evaluate((node) => {
    const gl = node.getContext('webgl2');
    const pixels = new Uint8Array(node.width * node.height * 4);
    gl.readPixels(0, 0, node.width, node.height, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
    const scale = Math.min(node.clientWidth / 700, node.clientHeight / 600);
    const logicalWidth = node.clientWidth / scale;
    const logicalHeight = node.clientHeight / scale;
    const densityX = node.width / node.clientWidth;
    const densityY = node.height / node.clientHeight;
    let hash = 0;
    let samples = 0;
    let saturated = 0;
    let bright = 0;
    let light = 0;
    for (let py = 0; py < node.height; py += 2) {
      const logicalY = (node.height - 1 - py) / (scale * densityY);
      if (logicalY < 105 || logicalY > logicalHeight - 75) continue;
      for (let px = 0; px < node.width; px += 2) {
        const logicalX = px / (scale * densityX);
        if (logicalX > 30 && logicalX < logicalWidth - 30) continue;
        const index = (py * node.width + px) * 4;
        const red = pixels[index];
        const green = pixels[index + 1];
        const blue = pixels[index + 2];
        const strongest = Math.max(red, green, blue);
        const weakest = Math.min(red, green, blue);
        hash = (hash * 33 + red * 3 + green * 5 + blue * 7) >>> 0;
        light += red + green + blue;
        samples += 1;
        if (strongest - weakest > 18 && strongest > 24) saturated += 1;
        if (strongest > 170) bright += 1;
      }
    }
    return {
      hash,
      samples,
      saturated,
      bright,
      averageChannel: light / Math.max(1, samples * 3),
    };
  });
}

async function expectAnimatedReadableSea(page, canvas) {
  await expect
    .poll(() => countCardBackInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(40);
  const before = await astralMarginMetrics(canvas);
  await page.waitForTimeout(700);
  const after = await astralMarginMetrics(canvas);

  expect(after.hash).not.toBe(before.hash);
  expect(after.saturated).toBeGreaterThan(after.samples * 0.16);
  expect(after.averageChannel).toBeGreaterThan(8);
  expect(after.averageChannel).toBeLessThan(72);
  expect(after.bright).toBeLessThan(after.samples * 0.04);
  expect(await countCardBackInk(canvas)).toBeGreaterThan(40);
}

test('animates a bold dark astral sea without losing duel readability', async ({ page }) => {
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  const canvas = await openGame(page);
  await beginHeist(page, canvas);
  await expectAnimatedReadableSea(page, canvas);

  await page.setViewportSize({ width: 600, height: 800 });
  await expect.poll(() => canvas.evaluate((node) => ({
    width: node.clientWidth,
    height: node.clientHeight,
  }))).toEqual({ width: 600, height: 800 });
  await expectAnimatedReadableSea(page, canvas);
  expect(pageErrors).toEqual([]);
});
