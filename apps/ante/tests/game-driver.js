import { expect } from '@playwright/test';

export const GAME_READY_TIMEOUT = 20_000;

export function frameSignature(canvas) {
  return canvas.evaluate((node) => {
    const gl = node.getContext('webgl2');
    const data = new Uint8Array(node.width * node.height * 4);
    gl.readPixels(0, 0, node.width, node.height, gl.RGBA, gl.UNSIGNED_BYTE, data);
    let hash = 0;
    for (let index = 0; index < data.length; index += 32) {
      hash = (hash * 33 + data[index] + data[index + 1] * 3 + data[index + 2] * 7) >>> 0;
    }
    return hash;
  });
}

// Card-back ink on the draw pile: only the heist screen draws the deck, so a
// positive count both proves the packaged asset decoded and that the menu has
// handed over to the game screen.
export function countCardBackInk(canvas) {
  return canvas.evaluate((node) => {
    const gl = node.getContext('webgl2');
    const pixels = new Uint8Array(104 * 128 * 4);
    gl.readPixels(56, node.height - 292 - 128, 104, 128, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
    let count = 0;
    for (let index = 0; index < pixels.length; index += 4) {
      if (
        Math.abs(pixels[index] - 232) <= 12
        && Math.abs(pixels[index + 1] - 223) <= 12
        && Math.abs(pixels[index + 2] - 189) <= 12
      ) count += 1;
    }
    return count;
  });
}

export async function openGame(page) {
  await page.goto('/');
  await expect(page.locator('h1')).toHaveText('Arcane Heist', { timeout: GAME_READY_TIMEOUT });
  return page.locator('canvas#walua-game-canvas');
}

// NEW GAME is the menu's default selection, so Enter activates it and opens
// the starting-spell list, where a second Enter takes the default FIREBOLT.
// The first Enter press is also the user gesture that unlocks browser audio.
export async function beginHeist(page, canvas) {
  await page.keyboard.press('Enter');
  await page.keyboard.press('Enter');
  await expect
    .poll(() => countCardBackInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(40);
}
