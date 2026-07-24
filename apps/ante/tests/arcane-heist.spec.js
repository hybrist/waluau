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

// Card-back ink on the draw pile: only the heist screen draws the deck, so a
// positive count both proves the packaged asset decoded and that the menu has
// handed over to the game screen.
function countCardBackInk(canvas) {
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

// Gold title ink in the band the menu's large "ARCANE HEIST" occupies. The
// heist screen keeps that band free of gold, so this distinguishes the menu
// from the game without depending on a perfectly still frame.
function countMenuTitleInk(canvas) {
  return canvas.evaluate((node) => {
    const width = Math.min(600, node.width - 300);
    const height = 120;
    const gl = node.getContext('webgl2');
    const pixels = new Uint8Array(width * height * 4);
    gl.readPixels(300, node.height - 280, width, height, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
    let count = 0;
    for (let index = 0; index < pixels.length; index += 4) {
      if (
        Math.abs(pixels[index] - 251) <= 20
        && Math.abs(pixels[index + 1] - 191) <= 20
        && Math.abs(pixels[index + 2] - 36) <= 20
      ) count += 1;
    }
    return count;
  });
}

async function openGame(page) {
  await page.goto('/');
  await expect(page.locator('h1')).toHaveText('Arcane Heist', { timeout: GAME_READY_TIMEOUT });
  return page.locator('canvas#walua-game-canvas');
}

// NEW GAME is the menu's default selection, so Enter activates it. The Enter
// press is also the user gesture that unlocks browser audio.
async function beginHeist(page, canvas) {
  await page.keyboard.press('Enter');
  await expect
    .poll(() => countCardBackInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(40);
}

test('boots to a menu with new game, boss battle, and how to play options', async ({ page }) => {
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  const canvas = await openGame(page);

  await expect
    .poll(() => countMenuTitleInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(300);

  // HOW TO PLAY (below NEW GAME and BOSS BATTLE) opens the shared help modal,
  // which covers the title band; Escape returns to the option list.
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await expect
    .poll(() => countMenuTitleInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeLessThan(50);
  await page.keyboard.press('Escape');
  await expect
    .poll(() => countMenuTitleInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(300);

  // Clicking the NEW GAME option starts the heist; M returns to the menu.
  const box = await canvas.boundingBox();
  await page.mouse.click(box.x + box.width * (480 / 960), box.y + box.height * (356 / 600));
  await expect
    .poll(() => countCardBackInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(40);
  await page.keyboard.press('m');
  await expect
    .poll(() => countMenuTitleInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(300);
  expect(pageErrors).toEqual([]);
});

test('starts a boss battle from its menu option', async ({ page }) => {
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  const canvas = await openGame(page);

  // BOSS BATTLE sits directly under NEW GAME; Enter on it deals the
  // eleven-card variant, whose board still shows the sealed draw pile.
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await expect
    .poll(() => countCardBackInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(40);
  expect(pageErrors).toEqual([]);
});

test('renders Arcane Heist and loads its packaged card-back asset', async ({ page }) => {
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  const canvas = await openGame(page);

  await expect(canvas).toBeVisible();
  await beginHeist(page, canvas);
  expect(pageErrors).toEqual([]);
});

test('responds to keyboard input without an iframe focus step', async ({ page }) => {
  const canvas = await openGame(page);
  await page.keyboard.press('Enter');
  await expect
    .poll(() => countCardBackInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(40);
});

test('previews four persistent card powers and resets them with key 0', async ({ page }) => {
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  const canvas = await openGame(page);

  await beginHeist(page, canvas);
  await page.keyboard.press('1');
  await page.keyboard.press('0');
  // Applying a power skips the opening deal, after which the fan still eases
  // into its resting pose. Capture the reset baseline only once that settles.
  await page.waitForTimeout(1_000);
  const baseline = await frameSignature(canvas);
  const signatures = [];
  for (const key of ['1', '2', '3', '4']) {
    await page.keyboard.press(key);
    await page.waitForTimeout(1_500);
    const signature = await frameSignature(canvas);
    expect(signatures).not.toContain(signature);
    signatures.push(signature);
  }

  await page.keyboard.press('0');
  await expect.poll(() => frameSignature(canvas)).toBe(baseline);
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

// Moved from the conformance runner: exercising Arcane Heist's packaged asset
// manifest is this app's concern, not the compiler's. The probe wraps the real
// AudioContext before the app boots so decode and playback are observable.
test('plays card flips through the packaged audio manifest only after the begin gesture', async ({ page }) => {
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  await page.addInitScript(() => {
    window.__audioProbe = { decodes: 0, starts: 0 };
    const RealAudioContext = window.AudioContext;
    window.AudioContext = class extends RealAudioContext {
      decodeAudioData(...args) {
        window.__audioProbe.decodes += 1;
        return super.decodeAudioData(...args);
      }
      createBufferSource() {
        const source = super.createBufferSource();
        const realStart = source.start.bind(source);
        source.start = (...args) => {
          window.__audioProbe.starts += 1;
          return realStart(...args);
        };
        return source;
      }
    };
  });
  const assetRequests = [];
  page.on('request', (request) => {
    if (request.url().includes('/assets/')) assetRequests.push(request.url());
  });

  const canvas = await openGame(page);
  const probe = () => page.evaluate(() => window.__audioProbe);
  await expect
    .poll(async () => (await probe()).decodes, { timeout: GAME_READY_TIMEOUT })
    .toBe(1);
  // Still on the menu: assets decode without any playback before the gesture.
  expect((await probe()).starts).toBe(0);

  await beginHeist(page, canvas);
  await expect
    .poll(async () => (await probe()).starts, { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(0);

  // The sound and font were served through their hashed manifest URLs. The
  // card back is small enough that the bundler inlines it — beginHeist's deck
  // ink poll already proves it decoded.
  expect(assetRequests.some((url) => /card-flip\..+\.wav$/.test(url))).toBe(true);
  expect(assetRequests.some((url) => /Cinzel-Bold\..+\.ttf$/.test(url))).toBe(true);
  expect(pageErrors).toEqual([]);
});

// Moved from the conformance runner: a missing flip sound must stop the app on
// its fatal audio diagnostic panel instead of playing on silently.
test('stops on the fatal audio diagnostic when the flip sound cannot load', async ({ page }) => {
  await page.route('**/*.wav', (route) => route.fulfill({ status: 404, body: '' }));
  const canvas = await openGame(page);
  const countFatalPanelInk = () => canvas.evaluate((node) => {
    const gl = node.getContext('webgl2');
    const data = new Uint8Array(node.width * node.height * 4);
    gl.readPixels(0, 0, node.width, node.height, gl.RGBA, gl.UNSIGNED_BYTE, data);
    let count = 0;
    for (let index = 0; index < data.length; index += 4) {
      if (
        Math.abs(data[index] - 38) <= 6
        && Math.abs(data[index + 1] - 11) <= 6
        && Math.abs(data[index + 2] - 8) <= 6
      ) count += 1;
    }
    return count;
  });
  await expect
    .poll(countFatalPanelInk, { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(5000);
});

test('plays a complete Arcane Heist game through the 2D engine', async ({ page }) => {
  test.slow();
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  const canvas = await openGame(page);

  await beginHeist(page, canvas);
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
