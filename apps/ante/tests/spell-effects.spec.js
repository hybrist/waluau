import { test, expect } from '@playwright/test';
import { GAME_READY_TIMEOUT, openGame, showTextControls } from './game-driver.js';

const status = (page) => page.getByRole('status');
const tableCards = (page) => page.getByRole('group', { name: 'Table cards', exact: true }).getByRole('button');
const handCards = (page) => page.getByRole('group', { name: 'Your hand', exact: true }).getByRole('button');

async function gold(page) {
  return Number((await status(page).textContent()).match(/Gold (\d+)\./)[1]);
}

async function deck(page) {
  return Number((await page.getByText(/^Deck: \d+ cards remaining\.$/).textContent()).match(/\d+/)[0]);
}

async function cardNames(cards) {
  return cards.evaluateAll((nodes) => nodes.map((node) => node.getAttribute('aria-label')));
}

async function startWithSpell(page, spell) {
  const canvas = await openGame(page);
  await showTextControls(page);
  await page.getByRole('button', { name: 'New run', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Starting vendor', exact: true })).toBeVisible();
  await page.getByRole('button', { name: spell, exact: true }).click();
  await expect(status(page)).toContainText('Board ready.', { timeout: GAME_READY_TIMEOUT });
  return canvas;
}

// Choreography appearance belongs to the spell Storybook scenes and visual
// suite (waluau-c0yh.6/.7). These tests observe costs, targets and rule effects.
test('canvas Firebolt targeting cancels freely and replaces only the confirmed card', async ({ page }) => {
  const canvas = await startWithSpell(page, 'Firebolt');
  const beforeGold = await gold(page);
  const beforeDeck = await deck(page);
  const beforeTable = await cardNames(tableCards(page));
  const beforeHand = await cardNames(handCards(page));
  // Collapse the panel opened during setup: this test drives the canvas.
  const toggle = page.getByRole('button', { name: 'Text controls', exact: true });
  await toggle.click();
  await canvas.focus();

  await page.keyboard.press('1');
  await expect(status(page)).toContainText('Firebolt targeting.');
  await page.keyboard.press('ArrowRight');
  await expect(status(page)).toContainText(`Target: ${beforeTable[1]}.`);
  await page.keyboard.press('Escape');
  await expect(status(page)).toContainText('Spell cancelled. No gold spent.');
  expect(await gold(page)).toBe(beforeGold);

  await page.keyboard.press('1');
  await expect(status(page)).toContainText('Firebolt targeting.');
  await page.keyboard.press('ArrowRight');
  await expect(status(page)).toContainText(`Target: ${beforeTable[1]}.`);
  await page.keyboard.press('Enter');
  await expect(status(page)).toContainText('Firebolt completed on table card 2.', { timeout: GAME_READY_TIMEOUT });
  await expect(status(page)).toContainText('Board ready.');
  expect(await gold(page)).toBe(beforeGold - 5);
  await expect(canvas).toBeFocused();
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');

  await showTextControls(page);
  const afterTable = await cardNames(tableCards(page));
  expect(afterTable[1]).not.toBe(beforeTable[1]);
  expect(afterTable.filter((_, index) => index !== 1)).toEqual(beforeTable.filter((_, index) => index !== 1));
  expect(await cardNames(handCards(page))).toEqual(beforeHand);
  expect(await deck(page)).toBe(beforeDeck - 1);
});

test('Freeze Ray charges once and prevents exchanging the frozen target', async ({ page }) => {
  await startWithSpell(page, 'Freeze Ray');
  const beforeGold = await gold(page);
  const beforeDeck = await deck(page);
  const target = tableCards(page).nth(1);
  const targetName = await target.getAttribute('aria-label');
  await page.getByRole('button', { name: 'Freeze Ray', exact: true }).click();
  await expect(status(page)).toContainText('Freeze Ray targeting.');
  await target.click();
  await expect(status(page)).toContainText('Freeze Ray completed on table card 2.');
  expect(await gold(page)).toBe(beforeGold - 5);
  expect(await deck(page)).toBe(beforeDeck);
  await expect(target).toHaveAccessibleName(`${targetName}, frozen`);
  await expect(target).toBeDisabled();
  await handCards(page).first().click();
  await expect(page.getByRole('button', { name: 'Confirm swap', exact: true })).toBeDisabled();
  // A retained DOM click from before the freeze cannot select it or charge.
  await target.dispatchEvent('click');
  await expect(target).toHaveAttribute('aria-pressed', 'false');
  await expect(page.getByRole('button', { name: 'Confirm swap', exact: true })).toBeDisabled();
  expect(await gold(page)).toBe(beforeGold - 5);
});

for (const spell of ['Raise Card', 'Clone']) {
  test(`${spell} spends gold for its described effect without changing cards`, async ({ page }) => {
    await startWithSpell(page, spell);
    const beforeGold = await gold(page);
    const beforeDeck = await deck(page);
    const beforeTable = await cardNames(tableCards(page));
    const beforeHand = await cardNames(handCards(page));
    const spellButton = page.getByRole('button', { name: spell, exact: true });
    await expect(spellButton).toHaveAttribute('aria-description', /stays unchanged/);
    await spellButton.click();
    await expect(status(page)).toContainText(`${spell} targeting.`);
    await tableCards(page).nth(2).click();
    await expect(status(page)).toContainText(`${spell} completed on table card 3.`);
    await expect(status(page)).toContainText('Board ready.');
    expect(await gold(page)).toBe(beforeGold - 5);
    expect(await deck(page)).toBe(beforeDeck);
    expect(await cardNames(tableCards(page))).toEqual(beforeTable);
    expect(await cardNames(handCards(page))).toEqual(beforeHand);
  });
}
