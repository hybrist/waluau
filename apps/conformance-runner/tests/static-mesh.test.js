import { expect, it } from 'vitest';
import { compileAndInstantiateWithDom } from '../src/runner.js';
import fixture from '../../../fixtures/game-engine/static-mesh.walu?raw';
import graphics from '../../../engine/graphics.walu?raw';
import resources from '../../../engine/resources.walu?raw';
import font from '../../../engine/font.walu?raw';

it('rejects invalid mesh draws, then depth-tests previews and restores 2D drawing', async () => {
  const { root, cleanup } = await compileAndInstantiateWithDom({
    '/fixtures/game-engine/static-mesh.walu': fixture,
    '/engine/graphics.walu': graphics,
    '/engine/resources.walu': resources,
    '/engine/font.walu': font,
  }, '/fixtures/game-engine/static-mesh.walu');
  try {
    const gl = root.querySelector('#static-mesh-test').getContext('webgl2');
    const pixel = (x, y) => {
      const rgba = new Uint8Array(4);
      gl.readPixels(x, 79 - y, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, rgba);
      return [...rgba];
    };
    // A nearer green surface survives a later red triangle in both cells.
    for (const x of [40, 120]) {
      const [red, green, blue] = pixel(x, 52);
      expect(red).toBeLessThan(2);
      expect(green).toBeGreaterThan(150);
      expect(blue).toBeLessThan(2);
    }
    // The fixture rejects released meshes and depthless-target draws via pcall.
    // Cyan proves the rejected draw left the offscreen target usable; the green
    // previews above prove valid screen mesh draws still work afterwards.
    expect(pixel(152, 72)).toEqual([0, 255, 255, 255]);
    expect(pixel(40, 40)).toEqual([255, 0, 255, 255]);
    expect(pixel(3, 3)).toEqual([255, 255, 0, 255]);
    expect(pixel(155, 3)).toEqual([0, 0, 255, 255]);
    expect(gl.isEnabled(gl.DEPTH_TEST)).toBe(false);
    expect(gl.isEnabled(gl.SCISSOR_TEST)).toBe(false);
    expect(gl.getError()).toBe(gl.NO_ERROR);
  } finally { cleanup(); }
}, 30_000);
