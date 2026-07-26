import { test, expect } from '@playwright/test';
import {
  GAME_READY_TIMEOUT,
  beginHeist,
  countCardBackInk,
  frameSignature,
  openGame,
} from './game-driver.js';

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

test('boots to a menu with new run, boss rush, and how to play options', async ({ page }) => {
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  const canvas = await openGame(page);

  await expect
    .poll(() => countMenuTitleInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(300);

  // HOW TO PLAY (below NEW RUN and BOSS RUSH) opens the shared help modal,
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

  // Clicking the NEW RUN option opens the starting-spell list; the same
  // top-row spot now names FIREBOLT, and clicking it starts the run. M
  // returns to the menu.
  const box = await canvas.boundingBox();
  await page.mouse.click(box.x + box.width * (480 / 960), box.y + box.height * (356 / 600));
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

test('starts a boss rush from its menu option', async ({ page }) => {
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  const canvas = await openGame(page);

  // BOSS RUSH sits directly under NEW RUN; Enter on it opens the spell list,
  // and a second Enter deals the first vault as the eleven-card variant,
  // whose board still shows the sealed draw pile.
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
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
  await page.keyboard.press('Enter');
  await expect
    .poll(() => countCardBackInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(40);
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

// Amber ink in the PASS capsule, which is drawn for exactly as long as the
// feint is open. It is how these tests tell the two halves of a breach apart
// without reaching into game state. Only the capsule's right half is sampled:
// the breach phase writes its own amber prompt across the band, and that
// prompt stops short of where the capsule ends.
function countPassInk(canvas) {
  return canvas.evaluate((node) => {
    const gl = node.getContext('webgl2');
    const left = Math.round(node.width * 0.612);
    const width = Math.round(node.width * 0.042);
    // readPixels counts up from the bottom; the capsule sits at 412..438 of
    // the 600-tall board.
    const bottom = Math.round(node.height * 0.27);
    const height = Math.round(node.height * 0.044);
    const pixels = new Uint8Array(width * height * 4);
    gl.readPixels(left, bottom, width, height, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
    let count = 0;
    for (let index = 0; index < pixels.length; index += 4) {
      if (
        Math.abs(pixels[index] - 251) <= 40
        && Math.abs(pixels[index + 1] - 191) <= 40
        && Math.abs(pixels[index + 2] - 36) <= 60
      ) count += 1;
    }
    return count;
  });
}

const PRESS_MS = 200;
const REVEAL_MS = 600;
const CLEANUP_MS = 1600;

// Two consecutive identical frames: the board has finished whatever it was
// animating and the next press will mean what it says rather than being spent
// skipping ahead.
async function settleBoard(canvas) {
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

// One breach played to its end: pass the feint, bind two relics, commit them,
// cut the reveal short, and clear it. The last press either refills for the
// next breach or raises the vault's verdict, depending on whether that breach
// ended the heist.
//
// Presses land only between animations, so passing keeps asking until the
// capsule is gone; a P that arrives mid-animation is spent skipping it, and
// one that arrives after the feint closed does nothing.
async function playBreach(page, canvas) {
  await expect
    .poll(async () => {
      await page.keyboard.press('p');
      await page.waitForTimeout(PRESS_MS);
      return countPassInk(canvas);
    }, { timeout: GAME_READY_TIMEOUT })
    .toBeLessThan(10);

  await settleBoard(canvas);
  await page.keyboard.press('Space');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('Space');
  await page.keyboard.press('Enter');
  await page.waitForTimeout(REVEAL_MS);
  await page.keyboard.press('Enter');
  await page.waitForTimeout(REVEAL_MS);
  await page.keyboard.press('Enter');
  await page.waitForTimeout(CLEANUP_MS);
}

test('carries the run into the next vault once this one is settled', async ({ page }) => {
  test.slow();
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  const canvas = await openGame(page);

  await beginHeist(page, canvas);

  // The verdict modal replaces the playfield, so the sealed draw pile stops
  // being drawn: no card-back ink means this vault has been settled, whether
  // the robbers took it or the Arch Mage held it.
  for (let breach = 1; breach <= 8; breach += 1) {
    if (await countCardBackInk(canvas) < 10) break;
    await playBreach(page, canvas);
  }
  await expect
    .poll(() => countCardBackInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeLessThan(10);

  // A settled vault is a moment in the run rather than the end of play: the
  // next press deals the vault the run moved to, sealed draw pile and all.
  await page.keyboard.press('Enter');
  await expect
    .poll(() => countCardBackInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(40);
  expect(pageErrors).toEqual([]);
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
