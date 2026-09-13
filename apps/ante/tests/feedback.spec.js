import { test, expect } from '@playwright/test';

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
  await toggle.click();
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
