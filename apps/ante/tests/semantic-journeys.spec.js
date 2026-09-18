import { test, expect } from '@playwright/test';
import { openGame, showTextControls } from './game-driver.js';

const status = (page) => page.getByRole('status');
const button = (page, name) => page.getByRole('button', { name, exact: true });
const group = (page, name) => page.getByRole('group', { name, exact: true });
const cards = (page, name) => group(page, name).getByRole('button');
const feedback = (page) => page.getByRole('region', { name: 'Game feedback', exact: true });

async function ready(page) {
  await expect(status(page)).toContainText('Board ready.', { timeout: 20_000 });
}

async function start(page, seed = 0x80000000, mode = 'New run') {
  // The host uses Math.random once to seed its production PRNG. No game state
  // or deck is injected; all subsequent changes are named player actions.
  await page.addInitScript((value) => { Math.random = () => value / 2 ** 32; }, seed);
  await openGame(page);
  await showTextControls(page);
  await button(page, mode).click();
  await button(page, 'Firebolt').click();
  await ready(page);
}

async function skipSwaps(page) {
  while (await button(page, 'Skip swap').count()) {
    await ready(page);
    const before = await cards(page, 'Table cards').count();
    await button(page, 'Skip swap').click();
    // Observe the action's effect before accepting readiness for the new phase.
    await expect.poll(async () => (await cards(page, 'Table cards').count()) !== before
      || (await button(page, 'Play hand').count()) > 0).toBe(true);
    await ready(page);
  }
}

async function playHand(page, names, preview, result) {
  await skipSwaps(page);
  const play = button(page, 'Play hand');
  await expect(play).toBeDisabled();
  for (const name of names) {
    const card = group(page, 'Your hand').getByRole('button', { name, exact: true });
    await card.click();
    await expect(card).toHaveAttribute('aria-pressed', 'true');
  }
  await expect(status(page)).toContainText(`Selected hand: ${preview}. Ready to play.`);
  await expect(play).toBeEnabled();
  await play.click();
  // Let normal reveal choreography finish; no reduced-motion preference,
  // screenshot comparison or fixed timing determines when Continue is legal.
  await expect(button(page, 'Continue')).toBeEnabled({ timeout: 20_000 });
  await expect(status(page)).toContainText(result);
  await button(page, 'Continue').click();
}

test('exchanges three real cards and charges their wager once', async ({ page }) => {
  // CI's software WebGL renderer can take 3–4 times the local elapsed time.
  // Keep each semantic readiness timeout bounded; budget the whole journey.
  test.setTimeout(90_000);
  await start(page);
  const outgoing = ['8 of red', '2 of blue', 'Ace of black'];
  const incoming = ['10 of blue', 'Jack of blue', '10 of red'];
  await expect(cards(page, 'Your hand')).toHaveText([...outgoing, '10 of black', '3 of green']);
  await expect(cards(page, 'Table cards')).toHaveText(incoming);
  await expect(feedback(page)).toContainText('Deck: 39 cards remaining.');
  const confirm = button(page, 'Confirm swap');
  await expect(confirm).toBeDisabled();
  for (const name of outgoing) await group(page, 'Your hand').getByRole('button', { name, exact: true }).click();
  await expect(confirm).toBeDisabled();
  for (const name of incoming) await group(page, 'Table cards').getByRole('button', { name, exact: true }).click();
  await expect(feedback(page)).toContainText('Selected hand cards: 3.');
  await expect(feedback(page)).toContainText('Selected table cards: 3.');
  await expect(confirm).toBeEnabled();
  await confirm.click();
  await expect(status(page)).toContainText('Gold 7.');
  await ready(page);
  for (const [index, name] of [...incoming, '10 of black', '3 of green'].entries()) {
    await expect(cards(page, 'Your hand').nth(index)).toHaveAccessibleName(name);
  }
  for (const name of outgoing) await expect(group(page, 'Table cards').getByRole('button', { name, exact: true })).toBeVisible();
  await expect(feedback(page)).toContainText('Pot 3. Your wager 3. Opponent wager 0.');
  await expect(feedback(page)).toContainText('Deck: 38 cards remaining.');
  await expect(feedback(page)).toContainText('Selected hand cards: 0.');
  await expect(confirm).toBeDisabled();
  await expect(status(page)).toContainText('Swap: select at least 4 matching cards');
});

test('buys a spell between duels, carries resources, then loses and starts a fresh run', async ({ page }) => {
  test.setTimeout(240_000);
  await start(page);
  await playHand(page, ['8 of red', '2 of blue'], 'three of a kind', 'Hand 1: Opponent wins.');
  await ready(page);
  await expect(status(page)).toContainText('Duel 1. Health 2. Gold 10.');
  await playHand(page, ['Ace of black', '10 of black'], 'straight', 'Hand 2: You win.');
  await expect(page.getByRole('dialog', { name: 'Duel verdict', exact: true })).toBeVisible();
  await expect(status(page)).toContainText('Duel cleared. Standard run. Standard duel. Duel 1. Health 2. Gold 21.');
  await button(page, 'Continue run').click();
  const drain = button(page, 'Drain Life (level 1), 4 gold');
  const mindRead = button(page, 'Mind Read (level 1), 4 gold');
  await expect(drain).toBeEnabled();
  await expect(mindRead).toBeEnabled();
  await expect(mindRead).toHaveAccessibleDescription('Reveal all cards currently in the opponent hand.');
  await mindRead.click();
  await expect(status(page)).toContainText('Health 2. Gold 17.');
  await expect(mindRead).toBeDisabled();
  // The two-spell loadout is full; the other kind cannot be purchased.
  await expect(drain).toBeDisabled();
  await button(page, 'Enter next duel').click();
  await ready(page);
  await expect(status(page)).toContainText('Duel 2. Health 2. Gold 17.');
  await expect(group(page, 'Spells').getByRole('button')).toHaveCount(2);
  await expect(group(page, 'Spells').getByRole('button', { name: 'Firebolt', exact: true })).toBeEnabled();
  await expect(group(page, 'Spells').getByRole('button', { name: 'Mind Read', exact: true })).toBeEnabled();
  await group(page, 'Spells').getByRole('button', { name: 'Mind Read', exact: true }).click();
  await expect(status(page)).toContainText('Duel 2. Health 2. Gold 12.');
  await expect(feedback(page)).toContainText('Opponent: 0 concealed cards.');
  await expect(feedback(page)).toContainText('Opponent card 1:');
  await expect(group(page, 'Spells').getByRole('button', { name: 'Mind Read', exact: true })).toBeDisabled();
  await playHand(page, ['8 of red', '10 of red'], 'pair', 'Hand 1: Opponent wins.');
  await ready(page);
  await expect(status(page)).toContainText('Duel 2. Health 1. Gold 12.');
  await playHand(page, ['4 of red', 'Jack of black'], 'pair', 'Hand 2: Opponent wins.');
  await expect(status(page)).toContainText('Run ended. Standard run. Standard duel. Duel 2. Health 0. Gold 12.');
  await expect(button(page, 'Continue run')).toHaveCount(0);
  await button(page, 'Start new run').click();
  await ready(page);
  await expect(status(page)).toContainText('Standard run. Standard duel. Duel 1. Health 3. Gold 10.');
  await expect(group(page, 'Spells').getByRole('button')).toHaveCount(1);
  await expect(group(page, 'Spells').getByRole('button').first()).toHaveAccessibleName('Firebolt');
  await expect(cards(page, 'Your hand')).toHaveCount(5);
  await expect(button(page, 'Skip swap')).toBeEnabled();
  await expect(feedback(page)).toContainText('Pot 0. Your wager 0. Opponent wager 0.');
});

test('plays all three boss hands before advancing to the next boss duel', async ({ page }) => {
  test.setTimeout(240_000);
  await start(page, 2, 'Boss rush');
  const context = 'Boss rush. Boss duel: Arch Mage.';
  await expect(status(page)).toContainText(`${context} Duel 1. Health 3. Gold 10.`);
  await expect(cards(page, 'Your hand')).toHaveCount(7);
  await playHand(page, ['3 of black', '2 of black'], 'flush', 'Hand 1: You win.');
  await ready(page);
  // Unlike a standard duel, an early win must leave this boss standing.
  await expect(status(page)).toContainText(`${context} Duel 1. Health 3. Gold 17.`);
  await expect(cards(page, 'Your hand')).toHaveCount(5);
  await expect(button(page, 'Continue run')).toHaveCount(0);
  await playHand(page, ['5 of green', 'Jack of green'], 'full house', 'Hand 2: You win.');
  await ready(page);
  await expect(cards(page, 'Your hand')).toHaveCount(3);
  await expect(status(page)).toContainText(`${context} Duel 1. Health 3. Gold 31.`);
  await playHand(page, ['8 of blue', 'Ace of red'], 'two pair', 'Hand 3: You win.');
  await expect(status(page)).toContainText(`Duel cleared. ${context} Duel 1. Health 3. Gold 36.`);
  await button(page, 'Continue run').click();
  await expect(status(page)).toContainText('Health 3. Gold 36.');
  await button(page, 'Enter next duel').click();
  await ready(page);
  await expect(status(page)).toContainText(`${context} Duel 2. Health 3. Gold 36.`);
  await expect(cards(page, 'Your hand')).toHaveCount(7);
  await expect(button(page, 'Skip swap')).toBeEnabled();
});
