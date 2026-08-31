import { test, expect } from '@playwright/test';
import {
  GAME_READY_TIMEOUT,
  beginHeist,
  clickMenuItem,
  countCardBackInk,
  countDesignInk,
  frameSignature,
  openGame,
} from './game-driver.js';

// Gold title ink in the band the menu's large "ANTE MAGIC" occupies. The
// heist screen keeps that band free of gold, so this distinguishes the menu
// from the game without depending on a perfectly still frame.
function countMenuTitleInk(canvas) {
  return countDesignInk(
    canvas,
    { centerOffsetX: -300, heightRatio: 0.3533333333, yOffset: -52, width: 600, height: 120 },
    [251, 191, 36],
    [20, 20, 20],
  );
}

// The cyan stall and gold party marker share the stop the run is standing on.
// Every map screen parks that stop 736 logical units in from the left and just
// past half the height (a height-driven canvas is always 600 units tall, so
// 306). The breathing party mote can cover the stall completely at one point
// in its cycle, so either layer proves the anchored stop is under the screen.
async function countMapStopInk(canvas) {
  const rect = { x: 710, y: 288, width: 56, height: 34 };
  const stall = await countDesignInk(canvas, rect, [103, 232, 249], [45, 45, 45]);
  const party = await countDesignInk(canvas, rect, [253, 230, 138], [35, 35, 35]);
  return stall + party;
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
  await clickMenuItem(page, canvas);
  await clickMenuItem(page, canvas);
  await expect
    .poll(() => countCardBackInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(40);
  await page.keyboard.press('m');
  await expect
    .poll(() => countMenuTitleInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(300);
  expect(pageErrors).toEqual([]);
});

test('picks the starting spell at a vendor on the city map, then dives into the first vault', async ({ page }) => {
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  const canvas = await openGame(page);
  await expect
    .poll(() => countMenuTitleInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(300);

  // The title drifts over the city with no route being walked, so the stop
  // anchor holds nothing yet. Opening the spell list pans to the vendor the run
  // sets out from and parks it beside the options, where it stays put.
  await page.keyboard.press('Enter');
  await expect
    .poll(() => countMapStopInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(30);

  // Taking the spell walks off that vendor and into the first vault: the deck
  // comes up and the map is no longer on the screen to be measured.
  await page.keyboard.press('Enter');
  await expect
    .poll(() => countCardBackInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(40);
  expect(await countMapStopInk(canvas)).toBeLessThan(10);
  expect(pageErrors).toEqual([]);
});

test('starts a boss rush from its menu option', async ({ page }) => {
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  const canvas = await openGame(page);

  // BOSS RUSH sits directly under NEW RUN; Enter on it opens the spell list,
  // and a second Enter deals the first vault as the seven-card variant,
  // whose board still shows the sealed draw pile.
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await page.keyboard.press('Enter');
  await expect
    .poll(() => countCardBackInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(40);
  expect(pageErrors).toEqual([]);
});

test('renders Ante Magic and loads its packaged card-back asset', async ({ page }) => {
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

test('reflows semantic layout and pointer targets through wide and tall canvases', async ({ page }) => {
  await page.setViewportSize({ width: 1200, height: 600 });
  const canvas = await openGame(page);
  await expect.poll(() => canvas.evaluate((node) => ({
    width: node.clientWidth,
    height: node.clientHeight,
  }))).toEqual({ width: 1200, height: 600 });

  // The title and option list are centered in the added width, and the first
  // option's live hit target advances to the spell stage.
  await expect
    .poll(() => countMenuTitleInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(300);
  await clickMenuItem(page, canvas);
  await page.setViewportSize({ width: 600, height: 800 });
  await expect.poll(() => canvas.evaluate((node) => ({
    width: node.clientWidth,
    height: node.clientHeight,
  }))).toEqual({ width: 600, height: 800 });

  // Tall space separates the heading, list, board rows, and footer. Clicking
  // the relocated first row starts the run; the deck then appears on the live
  // ward band rather than at a fixed 600-high board coordinate.
  await clickMenuItem(page, canvas);
  await expect
    .poll(() => countCardBackInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(40);
});

// Moved from the conformance runner: exercising Ante Magic's packaged asset
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

test('plays a complete Ante Magic game through the 2D engine', async ({ page }) => {
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
