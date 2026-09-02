import { test, expect } from '@playwright/test';
import {
  FIRST_SOCKET,
  GAME_READY_TIMEOUT,
  HELP_CONTROL,
  countAimPromptInk,
  countCardBackInk,
  countModalHeadingInk,
  foregroundSignature,
  openGame,
  tapDesignPoint,
  tapMenuItem,
} from './game-driver.js';

// This project runs on a tablet-shaped canvas with a touchscreen and no
// keyboard, so every gesture here is a finger. What it is really checking is
// that a run can be played that way at all: the engine hands touch contacts to
// the same callbacks a mouse uses, and the controls that used to be key hints
// are now things to tap.

// Poll bright card-and-chrome ink until two captures in a row match, so a board
// that is still dealing is never mistaken for a settled one while the dark
// astral sea continues moving beneath it.
async function settledSignature(page, canvas) {
  let settled = 0;
  await expect
    .poll(
      async () => {
        const before = await foregroundSignature(canvas);
        await page.waitForTimeout(400);
        const after = await foregroundSignature(canvas);
        settled = after;
        return before === after;
      },
      { timeout: GAME_READY_TIMEOUT },
    )
    .toBe(true);
  return settled;
}

test('reaches a live board from the menu with taps only', async ({ page }) => {
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  const canvas = await openGame(page);

  // NEW RUN, then FIREBOLT on the starting-spell list that replaces it. The
  // first tap is also the user gesture that unlocks browser audio.
  await tapMenuItem(page, canvas);
  await tapMenuItem(page, canvas);
  await expect
    .poll(() => countCardBackInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(40);
  expect(pageErrors).toEqual([]);
});

test('arms and calls off a spell by tapping its socket', async ({ page }) => {
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  const canvas = await openGame(page);
  await tapMenuItem(page, canvas);
  await tapMenuItem(page, canvas);
  await expect
    .poll(() => countCardBackInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(40);
  const baseline = await settledSignature(page, canvas);

  // The socket is the finger's version of the number key: it opens targeting,
  // and tapping the same socket again calls the aim off without spending gold.
  expect(await countAimPromptInk(canvas)).toBeLessThan(5);
  await tapDesignPoint(page, canvas, FIRST_SOCKET);
  await expect
    .poll(() => countAimPromptInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(30);
  await tapDesignPoint(page, canvas, FIRST_SOCKET);
  await expect
    .poll(() => countAimPromptInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeLessThan(5);
  expect(pageErrors).toEqual([]);
});

test('opens and closes the rules from the standing controls', async ({ page }) => {
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  const canvas = await openGame(page);
  await tapMenuItem(page, canvas);
  await tapMenuItem(page, canvas);
  await expect
    .poll(() => countCardBackInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(40);
  await settledSignature(page, canvas);

  // HELP raises the rules over the board, and a tap anywhere puts them away
  // again — the control answers wherever the duel has got to.
  expect(await countModalHeadingInk(canvas)).toBeLessThan(5);
  await tapDesignPoint(page, canvas, HELP_CONTROL);
  await expect
    .poll(() => countModalHeadingInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(50);
  await tapDesignPoint(page, canvas, { centerOffsetX: 0, y: 300 });
  await expect
    .poll(() => countModalHeadingInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeLessThan(5);
  expect(pageErrors).toEqual([]);
});
