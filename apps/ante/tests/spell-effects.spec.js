import { test, expect } from '@playwright/test';
import { beginHeist, frameSignature, openGame, settleBoard } from './game-driver.js';

test('casts the chosen spell at a targeted ward for mana', async ({ page }) => {
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  const canvas = await openGame(page);

  // The default spell pick is FIREBOLT; the board settles to a still frame
  // once the opening deal finishes. Every assertion below is measured against
  // that frame, so wait for the deal to have stopped rather than for a
  // duration long enough to have covered it — a baseline captured mid-deal is
  // a frame the board will never return to.
  await beginHeist(page, canvas);
  await settleBoard(canvas);
  const baseline = await frameSignature(canvas);

  // Key 1 opens targeting (the aim ring and prompt change the frame);
  // Escape cancels without charging mana, restoring the settled board.
  await page.keyboard.press('1');
  await expect.poll(() => frameSignature(canvas)).not.toBe(baseline);
  await page.keyboard.press('Escape');
  await expect.poll(() => frameSignature(canvas)).toBe(baseline);

  // Aiming at the middle ward and confirming burns it. The burn-and-replace
  // choreography visibly changes the frame, then the discard pile, deck
  // count, ward, and spent mana settle to a new still frame. Motion and
  // settling are polled rather than sampled at fixed instants: under CI's
  // software rasterizer the frame rate is too low for two captures 100ms
  // apart to be guaranteed to differ while a card is in flight.
  await page.keyboard.press('1');
  await page.keyboard.press('ArrowRight');
  const midFlight = await frameSignature(canvas);
  await page.keyboard.press('Enter');
  await expect.poll(() => frameSignature(canvas), { timeout: 10_000 }).not.toBe(midFlight);
  let resolved = 0;
  await expect
    .poll(
      async () => {
        const before = await frameSignature(canvas);
        await page.waitForTimeout(400);
        const after = await frameSignature(canvas);
        resolved = after;
        return before === after && after !== baseline;
      },
      { timeout: 10_000 },
    )
    .toBe(true);
  await page.waitForTimeout(300);
  expect(await frameSignature(canvas)).toBe(resolved);
  expect(pageErrors).toEqual([]);
});
