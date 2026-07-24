import { test, expect } from '@playwright/test';
import { beginHeist, frameSignature, openGame } from './game-driver.js';

test('casts the chosen spell at a targeted ward for mana', async ({ page }) => {
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  const canvas = await openGame(page);

  // The default spell pick is FIREBOLT; the board settles to a still frame
  // once the opening deal finishes.
  await beginHeist(page, canvas);
  await page.waitForTimeout(2_500);
  const baseline = await frameSignature(canvas);

  // Key 1 opens targeting (the aim ring and prompt change the frame);
  // Escape cancels without charging mana, restoring the settled board.
  await page.keyboard.press('1');
  await expect.poll(() => frameSignature(canvas)).not.toBe(baseline);
  await page.keyboard.press('Escape');
  await expect.poll(() => frameSignature(canvas)).toBe(baseline);

  // Aiming at the middle ward and confirming launches the persistent burn
  // effect there and deducts 5 mana, so the board never returns to baseline.
  await page.keyboard.press('1');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('Enter');
  await page.waitForTimeout(1_500);
  expect(await frameSignature(canvas)).not.toBe(baseline);
  expect(pageErrors).toEqual([]);
});
