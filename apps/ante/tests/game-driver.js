import { expect } from '@playwright/test';

export const GAME_READY_TIMEOUT = 20_000;

// Passive feedback observes real canvas input without opening native controls
// or changing focus. It remains accessible while visually clipped.
export function gameFeedback(page) {
  return page.getByRole('region', { name: 'Game feedback', exact: true });
}

export async function waitForCanvasMenu(page) {
  await expect(gameFeedback(page).getByRole('heading', { name: 'Main menu', exact: true }))
    .toBeAttached({ timeout: GAME_READY_TIMEOUT });
}

export async function waitForCanvasBoard(page) {
  const feedback = gameFeedback(page);
  await expect(feedback.getByRole('heading', { name: 'Duel', exact: true }))
    .toBeAttached({ timeout: GAME_READY_TIMEOUT });
  await expect(feedback.getByRole('status')).toContainText('Board ready.', { timeout: GAME_READY_TIMEOUT });
}

// Readiness comes from the same enabled controls a player uses. Animation
// continues normally; a moving card is never mistaken for a ready board.
export async function waitForBoardReady(canvas) {
  const page = canvas.page();
  await showTextControls(page);
  await expect(page.getByRole('heading', { name: 'Duel', exact: true })).toBeVisible({ timeout: GAME_READY_TIMEOUT });
  await expect(page.getByRole('group', { name: 'Your hand', exact: true }).getByRole('button').first())
    .toBeEnabled({ timeout: GAME_READY_TIMEOUT });
}

export async function showTextControls(page) {
  const toggle = page.getByRole('button', { name: 'Text controls', exact: true });
  if (await toggle.getAttribute('aria-expanded') !== 'true') await toggle.click();
}

export async function waitForMenu(canvas) {
  const page = canvas.page();
  await showTextControls(page);
  await expect(page.getByRole('heading', { name: 'Main menu', exact: true })).toBeVisible({ timeout: GAME_READY_TIMEOUT });
  await expect(page.getByRole('button', { name: 'New run', exact: true })).toBeEnabled();
  await canvas.focus();
}

// Where a point in Ante's live logical coordinates lands on the page. The
// anchors centerOffsetX, rightOffsetX and
// bottomOffsetY follow the board's own edges, which is where the standing
// controls and the ability sockets sit.
export function designPoint(canvas, spec) {
  return canvas.evaluate((node, point) => {
    const bounds = node.getBoundingClientRect();
    const scale = Math.min(bounds.width / 700, bounds.height / 600);
    const logicalWidth = bounds.width / scale;
    const logicalHeight = bounds.height / scale;
    let x = point.x ?? 0;
    let y = point.y ?? 0;
    if (point.centerOffsetX !== undefined) x = logicalWidth * 0.5 + point.centerOffsetX;
    if (point.rightOffsetX !== undefined) x = logicalWidth + point.rightOffsetX;
    if (point.bottomOffsetY !== undefined) y = logicalHeight + point.bottomOffsetY;
    return { x: bounds.x + x * scale, y: bounds.y + y * scale };
  }, spec);
}

export async function clickDesignPoint(page, canvas, x, y) {
  const point = await designPoint(canvas, { x, y });
  await page.mouse.click(point.x, point.y);
}

export async function tapDesignPoint(page, canvas, spec) {
  const point = await designPoint(canvas, spec);
  await page.touchscreen.tap(point.x, point.y);
}

function menuItemPoint(canvas, index) {
  return canvas.evaluate((node, itemIndex) => {
    const bounds = node.getBoundingClientRect();
    const scale = Math.min(bounds.width / 700, bounds.height / 600);
    const logicalHeight = bounds.height / scale;
    return {
      x: bounds.x + bounds.width * 0.5,
      y: bounds.y + (logicalHeight * 0.51 + itemIndex * 60 + 26) * scale,
    };
  }, index);
}

export async function clickMenuItem(page, canvas, index = 0) {
  const point = await menuItemPoint(canvas, index);
  await page.mouse.click(point.x, point.y);
}

export async function tapMenuItem(page, canvas, index = 0) {
  const point = await menuItemPoint(canvas, index);
  await page.touchscreen.tap(point.x, point.y);
}

// The standing controls take the header band's right corner in two rows, each
// row ending at the board's own right margin, so a point just inside that
// margin is the rightmost control of its row: HELP above, RESTART below.
export const HELP_CONTROL = { rightOffsetX: -52, y: 36 };
export const RESTART_CONTROL = { rightOffsetX: -52, y: 64 };

// The first ability socket, at the South position of the diamond standing in
// the board's bottom-right corner. A run always carries the spell it started
// with, so this socket is always occupied.
export const FIRST_SOCKET = { rightOffsetX: -79, bottomOffsetY: -32 };

export async function openGame(page) {
  await page.goto('/');
  await expect(page.locator('h1')).toHaveText('Ante Magic', { timeout: GAME_READY_TIMEOUT });
  return page.locator('canvas#walua-game-canvas');
}

export async function beginHeist(page, canvas) {
  await waitForMenu(canvas);
  await page.getByRole('button', { name: 'New run', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Starting vendor', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Firebolt', exact: true }).click();
  await waitForBoardReady(canvas);
  await canvas.focus();
}
