import { test, expect } from '@playwright/test';
import { designPoint, HELP_CONTROL, showTextControls } from './game-driver.js';

test('canvas spell feedback remains readable without opening controls or changing focus', async ({ page }) => {
  await page.goto('/');
  const feedback = page.getByRole('region', { name: 'Game feedback' });
  const status = feedback.getByRole('status');
  const canvas = page.locator('canvas').first();
  const toggle = page.getByRole('button', { name: 'Text controls', exact: true });
  await expect(feedback.getByRole('heading', { name: 'Main menu', exact: true })).toBeAttached();
  await canvas.focus();
  await page.keyboard.press('Enter');
  await expect(feedback.getByRole('heading', { name: 'Starting vendor' })).toBeAttached();
  await page.keyboard.press('Enter');
  await expect(status).toContainText('Board ready.', { timeout: 20_000 });
  const initial = await status.textContent();
  const gold = Number(initial.match(/Gold (\d+)/)[1]);
  await page.keyboard.press('1');
  await expect(status).toContainText('Firebolt targeting.');
  await page.keyboard.press('Escape');
  await expect(status).toContainText('Spell cancelled. No gold spent.');
  await expect(status).toContainText(`Gold ${gold}.`);
  await page.keyboard.press('1');
  await expect(status).toContainText('Firebolt targeting.');
  await page.keyboard.press('Enter');
  await expect(status).toContainText('Firebolt completed on table card 1.', { timeout: 20_000 });
  await expect(status).toContainText(`Gold ${gold - 5}.`);
  await expect(feedback).toContainText(/Deck: \d+ cards remaining/);
  await expect(canvas).toBeFocused();
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  await expect(feedback.getByRole('button')).toHaveCount(0);
  await expect(page.getByRole('status')).toHaveCount(1);
  await page.keyboard.press('h');
  await expect(feedback.getByRole('heading', { name: 'Ledger' })).toBeAttached();
  await expect(canvas).toBeFocused();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await page.keyboard.press('Escape');
  await expect(feedback.getByRole('heading', { name: 'Duel', exact: true })).toBeAttached();
  await showTextControls(page);
  await expect(page.getByRole('status')).toHaveCount(1);
  // No mutation means no repeated live announcement on the frame loop.
  const unchanged = await status.evaluate((node) => new Promise((resolve) => {
    let changes = 0;
    const observer = new MutationObserver(() => { changes++; });
    observer.observe(node, { childList: true, characterData: true, subtree: true });
    requestAnimationFrame(() => requestAnimationFrame(() => {
      observer.disconnect();
      resolve(changes);
    }));
  }));
  expect(unchanged).toBe(0);
});


for (const [heading, point] of [
  ['How to play', HELP_CONTROL],
  // History is the left chip in the bottom row of the standing controls.
  ['Ledger', { rightOffsetX: -145, y: 64 }],
]) {
  test(`canvas ${heading} waits for a pending Firebolt before opening`, async ({ page }) => {
    await page.goto('/');
    const feedback = page.getByRole('region', { name: 'Game feedback' });
    const status = feedback.getByRole('status');
    const canvas = page.locator('canvas#walua-game-canvas');
    await expect(feedback.getByRole('heading', { name: 'Main menu', exact: true })).toBeAttached();
    await canvas.focus();
    await page.keyboard.press('Enter');
    await expect(feedback.getByRole('heading', { name: 'Starting vendor', exact: true })).toBeAttached();
    await page.keyboard.press('Enter');
    await expect(status).toContainText('Board ready.', { timeout: 20_000 });
    const target = await designPoint(canvas, point);
    await page.keyboard.press('1');
    await expect(status).toContainText('Firebolt targeting.');
    await page.keyboard.press('Enter');
    await expect(status).toContainText('Firebolt resolving.');
    await page.mouse.click(target.x, target.y);
    // Waiting for completion, not merely the old heading, catches the next
    // rendered frame if the click incorrectly opens an information screen.
    await expect(status).toContainText('Firebolt completed on table card 1.', { timeout: 20_000 });
    await expect(feedback.getByRole('heading', { name: 'Duel', exact: true })).toBeAttached();
    await page.mouse.click(target.x, target.y);
    await expect(feedback.getByRole('heading', { name: heading, exact: true })).toBeAttached();
    await expect(page.getByRole('button', { name: 'Text controls', exact: true })).toHaveAttribute('aria-expanded', 'false');
  });
}
