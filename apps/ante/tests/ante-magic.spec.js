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

// Cyan stall ink where the city map stands the stop the run is on. Every map
// screen parks that stop 736 logical units in from the left and just past half
// the height (a height-driven canvas is always 600 units tall, so 306), and
// the run always sets out from a vendor, so a positive count is the map really
// being under the screen rather than a backdrop that happens to look like one.
function countMapStopInk(canvas) {
  return countDesignInk(
    canvas,
    { x: 710, y: 288, width: 56, height: 34 },
    [103, 232, 249],
    [45, 45, 45],
  );
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

// Amber ink in the swap phase prompt, which is drawn for exactly as long as
// the feint/swap is open. It is how these tests tell the two halves of a
// breach apart without reaching into game state.
function countPassInk(canvas) {
  return countDesignInk(
    canvas,
    { centerOffsetX: -160, wardOffsetY: -48, width: 80, height: 16 },
    [251, 191, 36],
    [40, 40, 60],
  );
}

// Cyan ink where the fence prints the mana on hand: a 24px number right-aligned
// at the top of its panel. Nothing else draws cyan that high — the board's own
// mana readout sits in the HUD above it, and the verdict's headings are gold —
// so this is how these tests tell the fence apart from a dealt vault.
function countFenceManaInk(canvas) {
  return countDesignInk(
    canvas,
    { x: 499, heightRatio: 0.1133333333, yOffset: 32, width: 58, height: 32 },
    [103, 232, 249],
    [40, 40, 40],
  );
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

  // A settled vault is a moment in the run rather than the end of play. Taking
  // it stops at the fence, where the carried mana buys spell upgrades before
  // the next vault is dealt; a lost run has neither mana nor loadout left to
  // spend, so its fresh first vault is dealt straight away. Which of the two
  // this vault ended as is the cards' to decide, so this covers both.
  //
  // The fence stands on the animating city map, so there is no still frame to
  // wait for here: what settles is which of the two screens came up.
  await page.keyboard.press('Enter');
  await expect
    .poll(
      async () => (await countFenceManaInk(canvas)) > 20
        || (await countCardBackInk(canvas)) > 40,
      { timeout: GAME_READY_TIMEOUT })
    .toBe(true);
  if (await countFenceManaInk(canvas) > 20) {
    // The fence keeps to a column so the map it stands on stays readable: the
    // vendor the run has reached is still anchored beside the offers.
    await expect
      .poll(() => countMapStopInk(canvas), { timeout: GAME_READY_TIMEOUT })
      .toBeGreaterThan(30);
    // The fence is a cursor-driven offer list, and Esc walks past every offer
    // into the vault the run is standing in front of.
    await page.keyboard.press('ArrowDown');
    await page.keyboard.press('Escape');
  }
  await expect
    .poll(() => countCardBackInk(canvas), { timeout: GAME_READY_TIMEOUT })
    .toBeGreaterThan(40);
  expect(pageErrors).toEqual([]);
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
