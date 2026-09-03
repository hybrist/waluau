import { expect } from '@playwright/test';

export const GAME_READY_TIMEOUT = 20_000;

export function frameSignature(canvas) {
  return canvas.evaluate((node) => {
    const gl = node.getContext('webgl2');
    const cssScale = Math.min(node.clientWidth / 700, node.clientHeight / 600);
    const logicalWidth = node.clientWidth / cssScale;
    const logicalHeight = node.clientHeight / cssScale;
    const extra = Math.max(0, logicalHeight - 600);
    const computerY = 116 + extra * 0.3;
    const playerY = logicalHeight - 91 - extra * 0.2;
    const wardY = ((computerY + 64 + playerY) * 0.5 + 12) - 64;
    const densityX = node.width / node.clientWidth;
    const densityY = node.height / node.clientHeight;

    const regions = [
      // Deck
      { x: 56 + 21, y: wardY + 29, width: 50, height: 70 },
      // Table cards (left, center, right)
      { x: logicalWidth * 0.5 - 116 - 25, y: wardY + 29, width: 50, height: 70 },
      { x: logicalWidth * 0.5 - 25, y: wardY + 29, width: 50, height: 70 },
      { x: logicalWidth * 0.5 + 116 - 25, y: wardY + 29, width: 50, height: 70 },
      // Player hand card 1 (played during trick commits)
      { x: logicalWidth * 0.5 - 70 - 14, y: playerY + 40, width: 28, height: 28 },
    ];

    let hash = 0;
    for (const region of regions) {
      const left = Math.round(region.x * cssScale * densityX);
      const top = Math.round(region.y * cssScale * densityY);
      const width = Math.max(1, Math.round(region.width * cssScale * densityX));
      const height = Math.max(1, Math.round(region.height * cssScale * densityY));
      const bottom = node.height - top - height;
      if (left < 0 || bottom < 0 || left + width > node.width || bottom + height > node.height) {
        continue;
      }
      const data = new Uint8Array(width * height * 4);
      gl.readPixels(left, bottom, width, height, gl.RGBA, gl.UNSIGNED_BYTE, data);
      for (let i = 0; i < data.length; i += 16) {
        hash = (hash * 33 + data[i] + data[i + 1] * 3 + data[i + 2] * 7) >>> 0;
      }
    }

    // Aim prompt ink count (Firebolt / spell targeting)
    const aimLeft = Math.round((logicalWidth * 0.5 - 160) * cssScale * densityX);
    const aimTop = Math.round((wardY - 51) * cssScale * densityY);
    const aimWidth = Math.max(1, Math.round(320 * cssScale * densityX));
    const aimHeight = Math.max(1, Math.round(16 * cssScale * densityY));
    const aimBottom = node.height - aimTop - aimHeight;
    let aimInk = 0;
    if (aimLeft >= 0 && aimBottom >= 0 && aimLeft + aimWidth <= node.width && aimBottom + aimHeight <= node.height) {
      const aimPixels = new Uint8Array(aimWidth * aimHeight * 4);
      gl.readPixels(aimLeft, aimBottom, aimWidth, aimHeight, gl.RGBA, gl.UNSIGNED_BYTE, aimPixels);
      for (let i = 0; i < aimPixels.length; i += 4) {
        if (
          Math.abs(aimPixels[i] - 239) <= 30 &&
          Math.abs(aimPixels[i + 1] - 68) <= 30 &&
          Math.abs(aimPixels[i + 2] - 68) <= 30
        ) {
          aimInk += 1;
        }
      }
    }
    hash = (hash * 33 + aimInk) >>> 0;

    // Modal heading ink count (Help / Rules / History)
    const modalLeft = Math.round((logicalWidth * 0.5 - 300) * cssScale * densityX);
    const modalTop = Math.round(30 * cssScale * densityY);
    const modalWidth = Math.max(1, Math.round(180 * cssScale * densityX));
    const modalHeight = Math.max(1, Math.round(40 * cssScale * densityY));
    const modalBottom = node.height - modalTop - modalHeight;
    let modalInk = 0;
    if (modalLeft >= 0 && modalBottom >= 0 && modalLeft + modalWidth <= node.width && modalBottom + modalHeight <= node.height) {
      const modalPixels = new Uint8Array(modalWidth * modalHeight * 4);
      gl.readPixels(modalLeft, modalBottom, modalWidth, modalHeight, gl.RGBA, gl.UNSIGNED_BYTE, modalPixels);
      for (let i = 0; i < modalPixels.length; i += 4) {
        if (
          Math.abs(modalPixels[i] - 251) <= 20 &&
          Math.abs(modalPixels[i + 1] - 191) <= 20 &&
          Math.abs(modalPixels[i + 2] - 36) <= 20
        ) {
          modalInk += 1;
        }
      }
    }
    hash = (hash * 33 + modalInk) >>> 0;

    return hash;
  });
}

// Two consecutive identical frames: the board has finished whatever it was
// animating and the next press will mean what it says rather than being spent
// skipping ahead. Only the board settles this way — the city map behind the
// menu and the shop pans for as long as it is up, and a reveal left standing
// burns its lost cards forever.
export async function settleBoard(canvas) {
  let previous = -1;
  await expect
    .poll(async () => {
      const current = await frameSignature(canvas);
      const stable = current === previous;
      previous = current;
      return stable;
    }, { timeout: GAME_READY_TIMEOUT })
    .toBe(true);
}

// Read a rectangle expressed in Ante's live logical coordinates. Anchors let
// assertions follow semantic regions as added width or height moves them.
//
// The scale and band formulas mirror layout.walu: the unit scale is the
// largest card size at which the packed board — 700 units across, 600 units
// down, both sums of the board's own content — fits the canvas, and any
// height beyond the packed board is shared between the bands.
export function countDesignInk(canvas, rect, color, tolerance) {
  return canvas.evaluate((node, sample) => {
    const gl = node.getContext('webgl2');
    const cssScale = Math.min(node.clientWidth / 700, node.clientHeight / 600);
    const logicalWidth = node.clientWidth / cssScale;
    const logicalHeight = node.clientHeight / cssScale;
    const extra = Math.max(0, logicalHeight - 600);
    const computerY = 116 + extra * 0.3;
    const playerY = logicalHeight - 91 - extra * 0.2;
    const wardY = ((computerY + 64 + playerY) * 0.5 + 12) - 64;
    const actionY = wardY + 120;
    const densityX = node.width / node.clientWidth;
    const densityY = node.height / node.clientHeight;
    let x = sample.rect.x ?? 0;
    let y = sample.rect.y ?? 0;
    if (sample.rect.centerOffsetX !== undefined) x = logicalWidth * 0.5 + sample.rect.centerOffsetX;
    if (sample.rect.rightOffsetX !== undefined) x = logicalWidth + sample.rect.rightOffsetX;
    if (sample.rect.heightRatio !== undefined) y = logicalHeight * sample.rect.heightRatio + (sample.rect.yOffset ?? 0);
    if (sample.rect.wardOffsetY !== undefined) y = wardY + sample.rect.wardOffsetY;
    if (sample.rect.actionOffsetY !== undefined) y = actionY + sample.rect.actionOffsetY;
    if (sample.rect.bottomOffsetY !== undefined) y = logicalHeight + sample.rect.bottomOffsetY;
    const left = Math.round(x * cssScale * densityX);
    const top = Math.round(y * cssScale * densityY);
    const width = Math.max(1, Math.round(sample.rect.width * cssScale * densityX));
    const height = Math.max(1, Math.round(sample.rect.height * cssScale * densityY));
    const bottom = node.height - top - height;
    const pixels = new Uint8Array(width * height * 4);
    gl.readPixels(left, bottom, width, height, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
    let count = 0;
    for (let index = 0; index < pixels.length; index += 4) {
      if (
        Math.abs(pixels[index] - sample.color[0]) <= sample.tolerance[0]
        && Math.abs(pixels[index + 1] - sample.color[1]) <= sample.tolerance[1]
        && Math.abs(pixels[index + 2] - sample.color[2]) <= sample.tolerance[2]
      ) count += 1;
    }
    return count;
  }, { rect, color, tolerance });
}

// Where a point in Ante's live logical coordinates lands on the page. The
// anchors mirror countDesignInk's: centerOffsetX, rightOffsetX and
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

// Card-back ink on the draw pile: only the heist screen draws the deck, so a
// positive count both proves the packaged asset decoded and that the menu has
// handed over to the game screen.
export function countCardBackInk(canvas) {
  return countDesignInk(
    canvas,
    { x: 56, wardOffsetY: 0, width: 92, height: 128 },
    [232, 223, 189],
    [35, 35, 35],
  );
}

// The aim prompt that replaces the phase prompt above the table cards while a
// spell is targeting, written in that spell's own colour. Firebolt's is red,
// and the amber prompt it replaces is far enough away in green and blue to
// tell the two apart inside this tolerance.
export function countAimPromptInk(canvas) {
  return countDesignInk(
    canvas,
    { centerOffsetX: -160, wardOffsetY: -51, width: 320, height: 16 },
    [239, 68, 68],
    [30, 30, 30],
  );
}

// The gold heading of whichever modal is up, in the panel's top-left corner.
// The board underneath writes no gold into that band: the live round's amber
// orb ring is centred, well right of it.
export function countModalHeadingInk(canvas) {
  return countDesignInk(
    canvas,
    { centerOffsetX: -300, y: 30, width: 180, height: 40 },
    [251, 191, 36],
    [20, 20, 20],
  );
}

export async function openGame(page) {
  await page.goto('/');
  await expect(page.locator('h1')).toHaveText('Ante Magic', { timeout: GAME_READY_TIMEOUT });
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
