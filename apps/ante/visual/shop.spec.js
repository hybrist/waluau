import { test, expect } from '@playwright/test';
import { showTextControls } from '../tests/game-driver.js';

const status = (page) => page.getByRole('status');

async function openShop(page, args = '') {
  await page.goto(`/iframe.html?id=shop--sandbox&viewMode=story&args=${encodeURIComponent(args)}`);
  await expect(page.locator('body')).toHaveAttribute('data-ante-story-ready', 'true');
  await showTextControls(page);
  await expect(page.getByRole('heading', { name: 'Vendor', exact: true })).toBeVisible();
  return page.locator('canvas');
}

test('sandbox stock can be bought and replayed', async ({ page }) => {
  const canvas = await openShop(page);
  const scroll = page.getByRole('button', { name: 'Firebolt (level 2), 4 gold', exact: true });
  const potion = page.getByRole('button', { name: 'Healing Potion (+1 heart), 6 gold', exact: true });
  await expect(scroll).toBeEnabled();
  await expect(potion).toBeEnabled();

  await scroll.click();
  await expect(status(page)).toContainText('Gold 16.');
  await expect(scroll).toBeDisabled();

  await canvas.focus();
  await page.keyboard.press('r');
  await showTextControls(page);
  await expect(status(page)).toContainText('Gold 20.');
  await expect(scroll).toBeEnabled();
});

test('sandbox configures zero to four known spells and every offer', async ({ page }) => {
  await openShop(
    page,
    'gold:17;known spell 1:5;known spell 1 level:2;known spell 2:6;known spell 2 level:3;known spell 3:7;known spell 3 level:4;known spell 4:8;known spell 4 level:5;offer 1:6;offer 2:7',
  );
  await expect(page.getByRole('button', { name: 'Wither (level 5), 4 gold', exact: true })).toBeEnabled();
  await expect(page.getByRole('button', { name: 'Mind Read (level 6), 4 gold', exact: true })).toBeEnabled();

  await page.evaluate(() => {
    window.__STORYBOOK_ADDONS_CHANNEL__.emit('updateStoryArgs', {
      storyId: 'shop--sandbox',
      updatedArgs: {
        gold: 0,
        hearts: 3,
        'known spell 1': 0,
        'known spell 2': 0,
        'known spell 3': 0,
        'known spell 4': 0,
        'offer 1': 11,
        'offer 2': 12,
      },
    });
  });
  const unknown = page.getByRole('button', { name: 'Rank Change (level 1), 4 gold', exact: true });
  await expect(unknown).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Healing Potion (+1 heart), 6 gold', exact: true })).toBeDisabled();
  await expect(status(page)).toContainText('Gold 0.');

  for (const [value, name] of [
    [0, 'Firebolt'], [1, 'Freeze Ray'], [2, 'Raise Card'], [3, 'Clone'],
    [4, 'Drain Life'], [5, 'Second Wind'], [6, 'Wither'], [7, 'Mind Read'],
    [8, 'Tornado'], [9, 'Oblivion'], [10, 'Color Change'], [11, 'Rank Change'],
  ]) {
    await page.evaluate(({ offer }) => {
      window.__STORYBOOK_ADDONS_CHANNEL__.emit('updateStoryArgs', {
        storyId: 'shop--sandbox', updatedArgs: { 'offer 1': offer },
      });
    }, { offer: value });
    await expect(page.getByRole('button', { name: `${name} (level 1), 4 gold`, exact: true })).toBeAttached();
  }
});
