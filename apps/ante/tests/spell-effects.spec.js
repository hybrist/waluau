import { test, expect } from '@playwright/test';
import {
  beginHeist,
  countActionPromptInk,
  countAimPromptInk,
  openGame,
  settleBoard,
} from './game-driver.js';

test('casts the chosen spell at a targeted ward for mana', async ({ page }) => {
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  const canvas = await openGame(page);

  // The default spell pick is FIREBOLT; the board settles to its live action
  // prompt once the opening deal finishes.
  await beginHeist(page, canvas);
  await settleBoard(canvas);

  // Key 1 opens targeting (the aim prompt appears in the spell's color);
  // Escape cancels without charging mana, restoring the live action prompt.
  expect(await countAimPromptInk(canvas)).toBeLessThan(5);
  await page.keyboard.press('1');
  await expect.poll(() => countAimPromptInk(canvas)).toBeGreaterThan(30);
  await page.keyboard.press('Escape');
  await expect.poll(() => countAimPromptInk(canvas)).toBeLessThan(5);
  expect(await countActionPromptInk(canvas)).toBeGreaterThan(15);

  // Aiming at the middle ward and confirming burns it. The burn-and-replace
  // choreography resolves, then the board settles back to the action prompt.
  await page.keyboard.press('1');
  await expect.poll(() => countAimPromptInk(canvas)).toBeGreaterThan(30);
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('Enter');
  await expect.poll(() => countAimPromptInk(canvas), { timeout: 10_000 }).toBeLessThan(5);
  await settleBoard(canvas);
  expect(await countActionPromptInk(canvas)).toBeGreaterThan(15);
  expect(pageErrors).toEqual([]);
});
