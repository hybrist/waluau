import { test, expect } from '@playwright/test';
import {
  GAME_READY_TIMEOUT,
  beginHeist,
  clickMenuItem,
  gameFeedback,
  designPoint,
  FIRST_SOCKET,
  HELP_CONTROL,
  waitForCanvasMenu,
  waitForCanvasBoard,
  openGame,
  waitForBoardReady,
  showTextControls,
  waitForMenu,
} from './game-driver.js';

test('keeps the splash up until the display font is ready', async ({ page }) => {
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  let releaseFont;
  let reportFontRequest;
  const fontReleased = new Promise((resolve) => { releaseFont = resolve; });
  const fontRequested = new Promise((resolve) => { reportFontRequest = resolve; });
  await page.route('**/*.ttf', async (route) => {
    reportFontRequest();
    await fontReleased;
    await route.continue();
  });
  const canvas = await openGame(page);
  try {
    await fontRequested;
    await showTextControls(page);
    await expect(page.getByRole('heading', { name: 'Loading Ante Magic', exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'New run', exact: true })).toHaveCount(0);
    // Canvas input while loading must not activate the menu behind the splash.
    await canvas.focus();
    await page.keyboard.press('Enter');
    await page.keyboard.press('Enter');
  } finally {
    releaseFont();
  }
  await waitForMenu(canvas);
  await expect(page.getByRole('heading', { name: 'Duel', exact: true })).toHaveCount(0);
  expect(pageErrors).toEqual([]);
});

test('leaves the splash for the built-in fallback when the display font fails', async ({ page }) => {
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  await page.route('**/*.ttf', (route) => route.fulfill({ status: 404, body: '' }));
  const canvas = await openGame(page);
  await waitForMenu(canvas);
  expect(pageErrors).toEqual([]);
});

test('opens help and returns to the menu before starting a run', async ({ page }) => {
  const canvas = await openGame(page);
  await waitForMenu(canvas);
  for (const name of ['New run', 'Boss rush', 'How to play']) {
    await expect(page.getByRole('button', { name, exact: true })).toBeEnabled();
  }
  await page.getByRole('button', { name: 'How to play', exact: true }).click();
  const help = page.getByRole('dialog', { name: 'How to play', exact: true });
  await expect(help).toBeVisible();
  await expect(help).toContainText('Swap: select equal numbers');
  await help.getByRole('button', { name: 'Close help', exact: true }).click();
  await waitForMenu(canvas);
  await beginHeist(page, canvas);
  await page.getByRole('button', { name: 'Main menu', exact: true }).click();
  await waitForMenu(canvas);
});

test('chooses a starting spell at the vendor and enters the first duel', async ({ page }) => {
  const canvas = await openGame(page);
  await waitForMenu(canvas);
  await page.getByRole('button', { name: 'New run', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Starting vendor', exact: true })).toBeVisible();
  for (const name of ['Firebolt', 'Freeze Ray', 'Raise Card', 'Clone']) {
    await expect(page.getByRole('button', { name, exact: true })).toBeEnabled();
  }
  await page.getByRole('button', { name: 'Back to menu', exact: true }).click();
  await waitForMenu(canvas);
  await beginHeist(page, canvas);
  await expect(page.getByRole('status')).toContainText('Duel 1.');
  await expect(page.getByRole('button', { name: /Firebolt/ })).toBeEnabled();
});

test('starts a boss rush with seven cards from its menu option', async ({ page }) => {
  const canvas = await openGame(page);
  await waitForMenu(canvas);
  await page.getByRole('button', { name: 'Boss rush', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Starting vendor', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Firebolt', exact: true }).click();
  await waitForBoardReady(canvas);
  await expect(page.getByRole('group', { name: 'Your hand', exact: true }).getByRole('button')).toHaveCount(7);
});

test('boots the canvas and completes asset loading before a playable duel', async ({ page }) => {
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  const canvas = await openGame(page);
  await expect(canvas).toBeVisible();
  await beginHeist(page, canvas);
  expect(pageErrors).toEqual([]);
});

test('routes canvas keyboard selection, targeting, cancellation and help', async ({ page }) => {
  const canvas = await openGame(page);
  const feedback = gameFeedback(page);
  const status = feedback.getByRole('status');
  await waitForCanvasMenu(page);
  await canvas.focus();
  await page.keyboard.press('Enter');
  await expect(feedback.getByRole('heading', { name: 'Starting vendor', exact: true })).toBeAttached();
  await page.keyboard.press('Enter');
  await waitForCanvasBoard(page);
  await page.keyboard.press('Space');
  await expect(feedback).toContainText('Selected hand cards: 1.');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('Space');
  await expect(feedback).toContainText('Selected hand cards: 2.');
  await page.keyboard.press('Space');
  await expect(feedback).toContainText('Selected hand cards: 1.');
  await page.keyboard.press('1');
  await expect(status).toContainText('Firebolt targeting.');
  const target = (await status.textContent()).match(/Target: (.+?)\./)[1];
  await page.keyboard.press('ArrowRight');
  await expect(status).toContainText('Firebolt targeting. Choose a table card. Target:');
  await expect(status).not.toContainText(`Target: ${target}.`);
  await page.keyboard.press('Escape');
  await expect(status).toContainText('Spell cancelled. No gold spent.');
  await page.keyboard.press('?');
  await expect(feedback.getByRole('heading', { name: 'How to play', exact: true })).toBeAttached();
  await page.keyboard.press('Escape');
  await waitForCanvasBoard(page);
  await expect(canvas).toBeFocused();
  await expect(page.getByRole('button', { name: 'Text controls', exact: true })).toHaveAttribute('aria-expanded', 'false');
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

test('routes pointer targets through wide and tall canvases', async ({ page }) => {
  await page.setViewportSize({ width: 1200, height: 600 });
  const canvas = await openGame(page);
  await waitForCanvasMenu(page);
  await expect.poll(() => canvas.evaluate((node) => ({ width: node.clientWidth, height: node.clientHeight })))
    .toEqual({ width: 1200, height: 600 });
  await clickMenuItem(page, canvas);
  await expect(gameFeedback(page).getByRole('heading', { name: 'Starting vendor', exact: true })).toBeAttached();
  await page.setViewportSize({ width: 600, height: 800 });
  await expect.poll(() => canvas.evaluate((node) => ({ width: node.clientWidth, height: node.clientHeight })))
    .toEqual({ width: 600, height: 800 });
  await clickMenuItem(page, canvas);
  await waitForCanvasBoard(page);
  for (const viewport of [{ width: 600, height: 800 }, { width: 1200, height: 600 }]) {
    await page.setViewportSize(viewport);
    await expect.poll(() => canvas.evaluate((node) => ({ width: node.clientWidth, height: node.clientHeight })))
      .toEqual(viewport);
    const socket = await designPoint(canvas, FIRST_SOCKET);
    await page.mouse.click(socket.x, socket.y);
    await expect(gameFeedback(page).getByRole('status')).toContainText('Firebolt targeting.');
    await page.mouse.click(socket.x, socket.y);
    await expect(gameFeedback(page).getByRole('status')).toContainText('Spell cancelled. No gold spent.');
    const help = await designPoint(canvas, HELP_CONTROL);
    await page.mouse.click(help.x, help.y);
    await expect(gameFeedback(page).getByRole('heading', { name: 'How to play', exact: true })).toBeAttached();
    const center = await designPoint(canvas, { centerOffsetX: 0, y: 300 });
    await page.mouse.click(center.x, center.y);
    await waitForCanvasBoard(page);
  }
  await expect(page.getByRole('button', { name: 'Text controls', exact: true })).toHaveAttribute('aria-expanded', 'false');
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

  // Assets were served through their hashed manifest URLs. Readiness covers
  // loading; the visual suite checks the rendered card back.
  expect(assetRequests.some((url) => /card-flip\..+\.wav$/.test(url))).toBe(true);
  expect(assetRequests.some((url) => /Cinzel-Bold\..+\.ttf$/.test(url))).toBe(true);
  expect(assetRequests.some((url) => /card-back\..+\.png$/.test(url))).toBe(true);
  expect(pageErrors).toEqual([]);
});

// Moved from the conformance runner: a missing flip sound must stop the app on
// its fatal audio diagnostic panel instead of playing on silently.
test('stops on the fatal audio diagnostic when the flip sound cannot load', async ({ page }) => {
  await page.route('**/*.wav', (route) => route.fulfill({ status: 404, body: '' }));
  await openGame(page);
  await showTextControls(page);
  await expect(page.getByRole('heading', { name: 'Audio could not load', exact: true })).toBeVisible();
  await expect(page.getByRole('status')).toContainText('Ante Magic cannot continue:');
});

// The complete journey follows the same named controls and readiness that a
// player uses, without reading GPU pixels or injecting private game state.
test('plays a duel through accessible controls, verdict, ledger, and restart', async ({ page }) => {
  test.slow();
  const pageErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  await page.addInitScript(() => { Math.random = () => 0.5; });
  await openGame(page);
  await showTextControls(page);
  await page.getByRole('button', { name: 'New run', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Starting vendor', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Firebolt', exact: true }).click();

  let handsPlayed = 0;
  while (handsPlayed < 8) {
    // A reveal dismissal either deals another hand or opens the verdict.
    await expect(page.getByRole('heading', { name: /^(Duel|Duel verdict)$/ })).toBeVisible();
    if (await page.getByRole('dialog', { name: 'Duel verdict', exact: true }).count()) break;
    const skip = page.getByRole('button', { name: 'Skip swap', exact: true });
    while (await skip.count()) {
      const table = page.getByRole('group', { name: 'Table cards', exact: true });
      const before = await table.getByRole('button').count();
      await skip.click();
      // First observe the exchange's result, then its readiness. Waiting only
      // for idle text could accidentally accept the previous frame's status.
      await expect.poll(async () => (await table.getByRole('button').count()) !== before
        || (await page.getByRole('button', { name: 'Play hand', exact: true }).count()) > 0).toBe(true);
      await expect(page.getByRole('status')).not.toContainText('Cards are moving');
    }
    const hand = page.getByRole('group', { name: 'Your hand', exact: true });
    const first = hand.getByRole('button').nth(0);
    const second = hand.getByRole('button').nth(1);
    await first.click();
    await expect(first).toHaveAttribute('aria-pressed', 'true');
    await second.click();
    await expect(second).toHaveAttribute('aria-pressed', 'true');
    await page.getByRole('button', { name: 'Play hand', exact: true }).click();
    // Continue becomes enabled only once it dismisses the displayed result.
    await expect(page.getByRole('button', { name: 'Continue', exact: true })).toBeEnabled();
    await expect(page.getByRole('status')).toContainText(/You win|Opponent wins|Tie/);
    await page.getByRole('button', { name: 'Continue', exact: true }).click();
    handsPlayed += 1;
  }
  expect(handsPlayed).toBeGreaterThan(0);
  const verdict = page.getByRole('dialog', { name: 'Duel verdict', exact: true });
  await expect(verdict).toBeVisible();
  const ledgerButton = verdict.getByRole('button', { name: 'Open ledger', exact: true });
  await ledgerButton.click();
  const ledger = page.getByRole('dialog', { name: 'Ledger', exact: true });
  await expect(ledger).toBeVisible();
  await expect(ledger.getByRole('group')).toHaveCount(handsPlayed);
  await page.keyboard.press('Escape');
  await expect(verdict).toBeVisible();
  await expect(ledgerButton).toBeFocused();
  await verdict.getByRole('button', { name: 'Restart run', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Skip swap', exact: true })).toBeEnabled();
  await expect(page.getByRole('status')).toContainText('Duel 1.');
  expect(pageErrors).toEqual([]);
});

test('native keyboard activation keeps focus and selects each card once', async ({ page }) => {
  const canvas = await openGame(page);
  const toggle = page.getByRole('button', { name: 'Text controls', exact: true });
  await toggle.focus();
  await page.keyboard.press('Enter');
  const start = page.getByRole('button', { name: 'New run', exact: true });
  await expect(start).toBeVisible();
  await toggle.focus();
  await page.keyboard.press('Tab');
  await expect(start).toBeFocused();
  await page.keyboard.press('Enter');
  // A duplicated Enter would also choose the vendor's default spell.
  await expect(page.getByRole('heading', { name: 'Starting vendor', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Firebolt', exact: true }).focus();
  await page.keyboard.press('Enter');
  const first = page.getByRole('group', { name: 'Your hand', exact: true }).getByRole('button').first();
  await expect(first).toBeEnabled();
  await first.focus();
  await page.keyboard.press('Space');
  await expect(first).toHaveAttribute('aria-pressed', 'true');
  await expect(first).toBeFocused();
  await page.keyboard.press('Space');
  await expect(first).toHaveAttribute('aria-pressed', 'false');
  await expect(first).toBeFocused();
  await toggle.focus();
  await page.keyboard.press('Enter');
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  // A click in the canvas margin resumes canvas input without selecting a card.
  await canvas.click({ position: { x: 5, y: 100 } });
  await expect(canvas).toBeFocused();
  await page.keyboard.press('Space');
  await expect(gameFeedback(page)).toContainText('Selected hand cards: 1.');
  await page.keyboard.press('Space');
  await expect(gameFeedback(page)).toContainText('Selected hand cards: 0.');
  await expect(canvas).toBeFocused();
});
