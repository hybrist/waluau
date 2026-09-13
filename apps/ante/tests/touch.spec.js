import { test, expect } from '@playwright/test';
import {
  FIRST_SOCKET,
  HELP_CONTROL,
  gameFeedback,
  openGame,
  tapDesignPoint,
  tapMenuItem,
  waitForCanvasBoard,
  waitForCanvasMenu,
} from './game-driver.js';

// Every action is a real finger gesture. Passive feedback never expands the
// text panel, changes focus, or intercepts the canvas hit targets.
async function beginWithTaps(page) {
  const canvas = await openGame(page);
  await waitForCanvasMenu(page);
  await tapMenuItem(page, canvas);
  await expect(gameFeedback(page).getByRole('heading', { name: 'Starting vendor', exact: true })).toBeAttached();
  await tapMenuItem(page, canvas);
  await waitForCanvasBoard(page);
  return canvas;
}

test.afterEach(async ({ page }) => {
  await expect(page.getByRole('button', { name: 'Text controls', exact: true })).toHaveAttribute('aria-expanded', 'false');
});

test('reaches a live board from the menu with taps only', async ({ page }) => {
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  await beginWithTaps(page);
  await expect(gameFeedback(page).getByRole('status')).toContainText('Standard run. Standard duel. Duel 1.');
  await expect(gameFeedback(page)).toContainText('Selected hand cards: 0.');
  expect(pageErrors).toEqual([]);
});

test('arms and calls off a spell by tapping its socket', async ({ page }) => {
  const canvas = await beginWithTaps(page);
  const status = gameFeedback(page).getByRole('status');
  const gold = (await status.textContent()).match(/Gold (\d+)\./)[1];
  await tapDesignPoint(page, canvas, FIRST_SOCKET);
  await expect(status).toContainText('Firebolt targeting. Choose a table card.');
  await tapDesignPoint(page, canvas, FIRST_SOCKET);
  await expect(status).toContainText('Spell cancelled. No gold spent.');
  await expect(status).toContainText(`Gold ${gold}.`);
  await expect(status).toContainText('Board ready.');
});

test('opens and closes the rules from the standing controls', async ({ page }) => {
  const canvas = await beginWithTaps(page);
  const feedback = gameFeedback(page);
  await tapDesignPoint(page, canvas, HELP_CONTROL);
  await expect(feedback.getByRole('heading', { name: 'How to play', exact: true })).toBeAttached();
  await expect(feedback).toContainText('Swap: select equal numbers');
  // The canvas modal dismisses on a tap anywhere, including its center.
  await tapDesignPoint(page, canvas, { centerOffsetX: 0, y: 300 });
  await waitForCanvasBoard(page);
});
