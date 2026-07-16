import { test, expect } from '@playwright/test';

const COMPILER_READY_TIMEOUT = 20_000;

test.describe('/output/poker-tricks', () => {
  test('renders Arcane Heist directly in the page, without the playground UI', async ({ page }) => {
    const pageErrors = [];
    page.on('pageerror', (error) => pageErrors.push(error.message));
    await page.goto('/output/poker-tricks');

    // The game renders into the top-level document: no iframe in sight.
    await expect(page.locator('h1')).toHaveText('Arcane Heist', {
      timeout: COMPILER_READY_TIMEOUT,
    });
    await expect(page.locator('iframe')).toHaveCount(0);
    await expect(page.locator('#example-error')).toHaveCount(0);

    // None of the playground chrome is present.
    await expect(page.locator('.code-textarea')).toHaveCount(0);
    await expect(page.locator('.status-text')).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Run' })).toHaveCount(0);
    await expect(page.locator('.dom-output-frame')).toHaveCount(0);

    const canvas = page.locator('canvas#walua-game-canvas');
    await expect(canvas).toHaveJSProperty('width', 960);
    await expect(canvas).toHaveJSProperty('height', 600);
    expect(pageErrors).toEqual([]);
  });

  test('drives the game from key presses without focusing a frame first', async ({ page }) => {
    await page.goto('/output/poker-tricks');
    const canvas = page.locator('canvas#walua-game-canvas');
    await expect(canvas).toHaveJSProperty('width', 960, { timeout: COMPILER_READY_TIMEOUT });

    const signature = () =>
      canvas.evaluate((node) => {
        const gl = node.getContext('webgl2');
        const data = new Uint8Array(node.width * node.height * 4);
        gl.readPixels(0, 0, node.width, node.height, gl.RGBA, gl.UNSIGNED_BYTE, data);
        let hash = 0;
        for (let i = 0; i < data.length; i += 32) {
          hash = (hash * 33 + data[i] + data[i + 1] * 3 + data[i + 2] * 7) >>> 0;
        }
        return hash;
      });

    // Key presses reach the example's document listeners with no iframe focus dance.
    const beforeHistory = await signature();
    await page.keyboard.press('h');
    await expect.poll(signature).not.toBe(beforeHistory);
  });
});

test.describe('/output/poker-tricks on high-DPI displays', () => {
  test.use({ deviceScaleFactor: 2, viewport: { width: 1200, height: 800 } });

  test('matches the WebGL backing buffer to CSS size and device density', async ({ page }) => {
    await page.goto('/output/poker-tricks');
    const canvas = page.locator('canvas#walua-game-canvas');
    await expect(canvas).toHaveJSProperty('width', 1920, { timeout: COMPILER_READY_TIMEOUT });

    const metrics = () =>
      canvas.evaluate((node) => {
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

    await expect.poll(metrics).toEqual({
      ratio: 2,
      cssWidth: 960,
      cssHeight: 600,
      width: 1920,
      height: 1200,
      drawingBufferWidth: 1920,
      drawingBufferHeight: 1200,
    });

    await page.setViewportSize({ width: 800, height: 600 });
    await expect.poll(metrics).toEqual({
      ratio: 2,
      cssWidth: 784,
      cssHeight: 490,
      width: 1568,
      height: 980,
      drawingBufferWidth: 1568,
      drawingBufferHeight: 980,
    });
  });
});
