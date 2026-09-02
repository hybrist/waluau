import { test, expect } from '@playwright/test';
import {
  beginHeist,
  countAimPromptInk,
  foregroundSignature,
  openGame,
  settleBoard,
} from './game-driver.js';

test('casts the chosen spell at a targeted ward for mana', async ({ page }) => {
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  const canvas = await openGame(page);

  // The default spell pick is FIREBOLT; the foreground settles once the
  // opening deal finishes even though the astral sea keeps moving. Every
  // assertion below is measured against that bright card-and-chrome ink, so a
  // baseline captured mid-deal can never be mistaken for the finished board.
  await beginHeist(page, canvas);
  await settleBoard(canvas);
  const baseline = await foregroundSignature(canvas);

  // Key 1 opens targeting; Escape cancels without charging mana, restoring
  // both the prompt and the settled foreground while the sea flows underneath.
  await page.keyboard.press('1');
  await expect.poll(() => countAimPromptInk(canvas)).toBeGreaterThan(30);
  await page.keyboard.press('Escape');
  await expect.poll(() => countAimPromptInk(canvas)).toBeLessThan(5);
  await expect.poll(() => foregroundSignature(canvas)).toBe(baseline);

  // Aiming at the middle ward and confirming burns it. The burn-and-replace
  // choreography visibly changes the foreground, then the discard pile, deck
  // count, ward, and spent mana settle to a new arrangement. Motion and
  // settling are polled rather than sampled at fixed instants: under CI's
  // software rasterizer the frame rate is too low for two captures 100ms
  // apart to be guaranteed to differ while a card is in flight.
  await page.keyboard.press('1');
  await page.keyboard.press('ArrowRight');
  await expect.poll(() => countAimPromptInk(canvas)).toBeGreaterThan(30);
  await page.keyboard.press('Enter');
  await expect.poll(() => countAimPromptInk(canvas), { timeout: 10_000 }).toBeLessThan(5);
  await expect.poll(() => foregroundSignature(canvas), { timeout: 10_000 }).not.toBe(baseline);
  let resolved = 0;
  await expect
    .poll(
      async () => {
        const before = await foregroundSignature(canvas);
        await page.waitForTimeout(400);
        const after = await foregroundSignature(canvas);
        resolved = after;
        return before === after && after !== baseline;
      },
      { timeout: 10_000 },
    )
    .toBe(true);
  await page.waitForTimeout(300);
  expect(await foregroundSignature(canvas)).toBe(resolved);
  expect(pageErrors).toEqual([]);
});
