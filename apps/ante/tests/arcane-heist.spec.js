import { test, expect } from '@playwright/test';

const GAME_READY_TIMEOUT = 20_000;

function frameSignature(canvas) {
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

async function openGame(page) {
  await page.goto('/');
  await expect(page.locator('h1')).toHaveText('Arcane Heist', { timeout: GAME_READY_TIMEOUT });
  return page.locator('canvas#walua-game-canvas');
}

test('renders Arcane Heist and loads its packaged card-back asset', async ({ page }) => {
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  const canvas = await openGame(page);

  await expect(canvas).toBeVisible();
  const countCardBackInk = () => canvas.evaluate((node) => {
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
  await expect.poll(countCardBackInk).toBeGreaterThan(40);
  expect(pageErrors).toEqual([]);
});

test('responds to keyboard input without an iframe focus step', async ({ page }) => {
  const canvas = await openGame(page);
  const beforeHistory = await frameSignature(canvas);
  await page.keyboard.press('h');
  await expect.poll(() => frameSignature(canvas)).not.toBe(beforeHistory);
});

test('previews four distinct persistent card powers with keys 1 through 4', async ({ page }) => {
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  const canvas = await openGame(page);

  await canvas.click();
  const signatures = [];
  for (const key of ['1', '2', '3', '4']) {
    await page.keyboard.press(key);
    await page.waitForTimeout(1_500);
    const signature = await frameSignature(canvas);
    expect(signatures).not.toContain(signature);
    signatures.push(signature);
  }

  expect(pageErrors).toEqual([]);
});

test.describe('on high-DPI displays', () => {
  test.use({ deviceScaleFactor: 2, viewport: { width: 1200, height: 800 } });

  test('matches its WebGL backing buffer to CSS size and device density', async ({ page }) => {
    const canvas = await openGame(page);
    const metrics = () => canvas.evaluate((node) => {
      const gl = node.getContext('webgl2');
      return {
        ratio: window.devicePixelRatio,
        cssWidth: node.clientWidth,
        cssHeight: node.clientHeight,
        width: node.width,
        height: node.height,
        drawingBufferWidth: gl.drawingBufferWidth,
        drawingBufferHeight: gl.drawingBufferHeight,
      };
    });

    await expect.poll(metrics).toMatchObject({
      ratio: 2,
      width: 2400,
      height: 1600,
      drawingBufferWidth: 2400,
      drawingBufferHeight: 1600,
    });

    await page.setViewportSize({ width: 800, height: 600 });
    await expect.poll(metrics).toMatchObject({
      ratio: 2,
      width: 1600,
      height: 1200,
      drawingBufferWidth: 1600,
      drawingBufferHeight: 1200,
    });
  });
});

test('plays a complete Arcane Heist game through the 2D engine', async ({ page }) => {
  test.slow();
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  const canvas = await openGame(page);

  await canvas.click();
  const playSignature = await frameSignature(canvas);
  await page.keyboard.press('h');
  await expect.poll(() => frameSignature(canvas)).not.toBe(playSignature);
  const historySignature = await frameSignature(canvas);
  await page.keyboard.press('h');
  await expect.poll(() => frameSignature(canvas)).not.toBe(historySignature);

  for (let round = 1; round <= 6; round += 1) {
    await page.keyboard.press('p');
    await page.keyboard.press('Space');
    await page.keyboard.press('ArrowRight');
    await page.keyboard.press('Space');
    const beforeReveal = await frameSignature(canvas);
    await page.keyboard.press('Enter');
    await page.keyboard.press('Enter');
    await expect.poll(() => frameSignature(canvas)).not.toBe(beforeReveal);
    await page.keyboard.press('Enter');
  }

  const finalSignature = await frameSignature(canvas);
  await page.keyboard.press('h');
  await expect.poll(() => frameSignature(canvas)).not.toBe(finalSignature);
  await page.keyboard.press('h');
  await page.keyboard.press('r');
  await expect.poll(() => frameSignature(canvas)).not.toBe(finalSignature);
  expect(pageErrors).toEqual([]);
});
