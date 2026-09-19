import { test, expect } from '@playwright/test';
import { showTextControls, designPoint, FIRST_SOCKET, RESTART_CONTROL } from '../tests/game-driver.js';

// Exercise the actual playable stories, using the same text controls as the
// game. These assertions cover scenario setup and input, not pixel baselines.
const status = (page) => page.getByRole('status');
const hand = (page) => page.getByRole('group', { name: 'Your hand', exact: true }).getByRole('button');
const table = (page) => page.getByRole('group', { name: 'Table cards', exact: true }).getByRole('button');
const labels = (cards) => cards.evaluateAll((nodes) => nodes.map((node) => node.getAttribute('aria-label')));

async function openDuel(page, story = 'sandbox', args = '') {
  await page.goto(`/iframe.html?id=duel--${story}&viewMode=story&args=${encodeURIComponent(args)}`);
  await expect(page.locator('body')).toHaveAttribute('data-ante-story-ready', 'true');
  await showTextControls(page);
  return page.locator('canvas');
}

async function canvasFocus(page, canvas) {
  const toggle = page.getByRole('button', { name: 'Text controls', exact: true });
  if (await toggle.getAttribute('aria-expanded') === 'true') await toggle.click();
  await canvas.focus();
}

for (const [story, spell] of [
  ['firebolt', 'Firebolt'], ['freeze-ray', 'Freeze Ray'], ['raise-card', 'Raise Card'], ['clone', 'Clone'],
]) {
  test(`${spell} preset can cast and replay its seeded duel`, async ({ page }) => {
    const errors = [];
    page.on('pageerror', (error) => errors.push(error.message));
    const canvas = await openDuel(page, story);
    await expect(status(page)).toContainText('Board ready.');
    const initialHand = await labels(hand(page));
    const initialTable = await labels(table(page));
    await page.getByRole('button', { name: spell, exact: true }).click();
    await table(page).nth(1).click();
    await expect(status(page)).toContainText(`${spell} completed on table card 2.`, { timeout: 15_000 });
    await expect(status(page)).toContainText('Gold 15.');
    const after = await labels(table(page));
    if (story === 'freeze-ray') {
      expect(after[1]).toBe(`${initialTable[1]}, frozen`);
      await expect(table(page).nth(1)).toBeDisabled();
    } else if (story === 'firebolt') {
      expect(after[1]).not.toBe(initialTable[1]);
      expect(after.filter((_, i) => i !== 1)).toEqual(initialTable.filter((_, i) => i !== 1));
    } else {
      expect(after).toEqual(initialTable);
    }
    expect(await labels(hand(page))).toEqual(initialHand);
    await canvasFocus(page, canvas);
    await page.keyboard.press('r');
    await showTextControls(page);
    await expect(status(page)).toContainText('Gold 20.');
    expect(await labels(hand(page))).toEqual(initialHand);
    expect(await labels(table(page))).toEqual(initialTable);
    expect(errors).toEqual([]);
  });
}

test('sandbox controls configure boss, level, target and targeting phase; canvas restart preserves setup', async ({ page }) => {
  const canvas = await openDuel(page, 'sandbox', 'spell:0;level:3;gold:37;duel:1;seed:2468;phase:1;target:2');
  await expect(status(page)).toContainText('Boss duel:');
  await expect(status(page)).toContainText('Gold 37.');
  await expect(status(page)).toContainText('Firebolt targeting.');
  await expect(status(page)).toContainText('Targets:');
  await expect(hand(page)).toHaveCount(7);
  const initial = await labels(table(page));
  const deckBefore = Number((await page.getByText(/^Deck: \d+ cards remaining\.$/).textContent()).match(/\d+/)[0]);
  await canvasFocus(page, canvas);
  await page.keyboard.press('Escape');
  const socket = await designPoint(canvas, FIRST_SOCKET);
  await page.mouse.click(socket.x, socket.y);
  await expect(status(page)).toContainText('Firebolt targeting.');
  await page.keyboard.press('Enter');
  await expect(status(page)).toContainText('Gold 32.');
  await expect(status(page)).toContainText('Board ready.', { timeout: 15_000 });
  await expect(page.getByText(`Deck: ${deckBefore - 3} cards remaining.`, { exact: true })).toBeAttached();
  const restart = await designPoint(canvas, RESTART_CONTROL);
  await page.mouse.click(restart.x, restart.y);
  await expect(status(page)).toContainText('Gold 37.');
  await expect(status(page)).toContainText('Firebolt targeting.');
  await showTextControls(page);
  expect(await labels(table(page))).toEqual(initial);
});

test('casting phase applies the selected spell to the authored target', async ({ page }) => {
  await openDuel(page, 'freeze-ray', 'phase:2;target:3;gold:25');
  await expect(status(page)).toContainText('Freeze Ray completed on table card 3.');
  await expect(status(page)).toContainText('Gold 20.');
  await expect(table(page).nth(2)).toHaveAccessibleName(/, frozen$/);
});

test('live Storybook controls rebuild the scenario and zero gold prevents casting', async ({ page }) => {
  await openDuel(page);
  await page.evaluate(() => {
    window.__STORYBOOK_ADDONS_CHANNEL__.emit('updateStoryArgs', {
      storyId: 'duel--sandbox', updatedArgs: { spell: 1, gold: 0, duel: 1, seed: 6789, phase: 2, target: 5 },
    });
  });
  await expect(status(page)).toContainText('Gold 0.');
  await expect(status(page)).toContainText('Board ready.');
  await expect(status(page)).not.toContainText('targeting.');
  await expect(status(page)).toContainText('Boss duel:');
  await showTextControls(page);
  await expect(hand(page)).toHaveCount(7);
  await expect(page.getByRole('button', { name: 'Freeze Ray', exact: true })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Firebolt', exact: true })).toHaveCount(0);
});

test('sandbox selects every shop-only spell', async ({ page }) => {
  // Eight complete story loads exercise the resource barrier in sequence. The
  // pinned CI renderer can take slightly longer than the suite's 30s default.
  test.setTimeout(60_000);
  for (const [value, name] of [
    [4, 'Drain Life'], [5, 'Second Wind'], [6, 'Wither'], [7, 'Mind Read'],
    [8, 'Tornado'], [9, 'Oblivion'], [10, 'Color Change'], [11, 'Rank Change'],
  ]) {
    await openDuel(page, 'sandbox', `spell:${value}`);
    await expect(page.getByRole('button', { name, exact: true })).toHaveCount(1);
    await expect(page.getByRole('button', { name: 'Firebolt', exact: true })).toHaveCount(0);
  }
});
