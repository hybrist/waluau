import { test, expect } from '@playwright/test';

// Wall time loads assets; only this clock advances presentation. Repeated
// resource-barrier draws use t=0, never frame equality or arbitrary delays.
async function openStory(page, id, args, milliseconds) {
  await page.addInitScript(() => {
    let nextId = 0;
    const frames = new Map();
    window.requestAnimationFrame = (callback) => {
      frames.set(++nextId, callback);
      return nextId;
    };
    window.cancelAnimationFrame = (id) => frames.delete(id);
    window.anteVisualFrame = (timestamp) => {
      const pending = [...frames.values()];
      frames.clear();
      for (const callback of pending) callback(timestamp);
    };
    // These fixtures currently use no randomized simulation, but pin the host
    // seed so adding randomized decoration cannot make captures depend on luck.
    let seed = 12345;
    Math.random = () => ((seed = (1664525 * seed + 1013904223) >>> 0) / 2 ** 32);
  });
  await page.goto(`/iframe.html?id=${id}&viewMode=story&args=${encodeURIComponent(args)}`);
  const canvas = page.locator('canvas');
  await expect(canvas).toBeVisible();
  await expect.poll(() => page.evaluate((procedural) => {
    window.anteVisualFrame(0);
    if (procedural) {
      // This scene has no asynchronous assets. Its opaque green market at
      // the focal point proves the offscreen city and lens have both drawn;
      // the initial dark clear alone cannot satisfy this readiness check.
      const gl = document.querySelector('canvas').getContext('webgl2');
      const pixel = new Uint8Array(4);
      gl.readPixels(gl.drawingBufferWidth / 2, gl.drawingBufferHeight / 2,
        1, 1, gl.RGBA, gl.UNSIGNED_BYTE, pixel);
      return pixel[1] > 50 && pixel[2] > 50 && pixel[3] === 255 ? 'true' : 'false';
    }
    return document.body.getAttribute('data-ante-story-ready');
  }, id === 'city-scrying--start-node')).toBe('true');
  await page.evaluate(async (time) => {
    await document.fonts.ready;
    // The initial draw uploads all packaged textures/fonts. Every subsequent
    // frame is one explicit simulation step, ending at the requested instant.
    for (let frame = 1; frame <= Math.round(time * 60 / 1000); frame++) {
      window.anteVisualFrame(frame * (1000 / 60));
    }
    const gl = document.querySelector('canvas').getContext('webgl2');
    if (!gl) throw new Error('Visual stories must render with real WebGL2');
    gl.finish();
  }, milliseconds);
  return canvas;
}

const scenes = [
  ['city-scrying', 'city-scrying--start-node', 'motion:0;spell phase:25', 0],
  ['city-scrying-close', 'city-scrying--start-node', 'motion:0;spell phase:25;zoom percent:400', 0],
  ['card-back', 'card--face-down', '', 0],
  ['court-atlas', 'card--every-court', '', 0],
  ['suit-shaders', 'card--every-color', '', 250],
  ['card-draw-mid-flight', 'card--drawn-from-the-deck', 'playback:1;phase:45', 250],
  ['hand-focused', 'hand--fan', 'focused:3;selected:1', 0],
  ['shop', 'entities-shop--open-visit', '', 0],
  ['main-menu', 'screens--main-menu', '', 0],
  ['starting-vendor', 'screens--starting-vendor', '', 0],
  ['help', 'screens--help', '', 0],
  ['wide-duel', 'screens--wide-duel', '', 0],
  ['tall-duel', 'screens--tall-duel', '', 0],
  ['targeting', 'screens--targeting', '', 0],
  ['firebolt-burning', 'screens--firebolt', '', 1600],
  ['loading', 'screens--loading', '', 0],
  ['fatal-audio', 'screens--fatal-audio', '', 0],
];
for (const [name, id, args, time] of scenes) {
  test(name, async ({ page }) => {
    const errors = [];
    page.on('pageerror', (error) => errors.push(error.message));
    const canvas = await openStory(page, id, args, time);
    // The fixture clock is already frozen at the authored instant. A locator
    // screenshot waits for rAF-driven element stability, which can stall when
    // the application owns that clock; clip the canvas without moving it.
    const clip = await canvas.boundingBox();
    expect(clip).not.toBeNull();
    expect(await page.screenshot({ clip, animations: 'allow' })).toMatchSnapshot(`${name}.png`);
    expect(errors).toEqual([]);
  });
}
