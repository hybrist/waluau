import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { checkGameplaySource, checkGameplayTree } from './check-gameplay.mjs';

const semanticTest = `
await canvas.focus();
await page.keyboard.press('Enter');
await expect(page.getByRole('status')).toContainText('Board ready.');
await expect.poll(() => canvas.evaluate(node => node.width)).toBe(1200);
await page.mouse.click(target.x, target.y);
`;

test('allows semantic polling, real input and canvas size contracts', () => {
  assert.deepEqual(checkGameplaySource(semanticTest, 'flow.spec.js'), []);
});

for (const mutation of [
  'gl.readPixels(0, 0, width, height, gl.RGBA, gl.UNSIGNED_BYTE, bytes);',
  "gl['readPixels'](0, 0, width, height, gl.RGBA, gl.UNSIGNED_BYTE, bytes);",
  'context.getImageData(0, 0, 1, 1);',
  'await page.screenshot();',
  'await expect(page).toHaveScreenshot();',
  'await expect(frame).toMatchSnapshot();',
  'await page.waitForTimeout(100);',
  'await new Promise(resolve => setTimeout(resolve, 100));',
  "import { frameSignature } from './old-driver.js';",
  "import { countAimPromptInk } from './old-driver.js';",
  "import { paint } from '../visual/fixtures.js';",
  "await import('../renderer/probe.js');",
]) {
  test(`rejects gameplay mutation: ${mutation}`, () => {
    assert.ok(checkGameplaySource(semanticTest + mutation, 'flow.spec.js').length > 0);
  });
}

test('ignores comments without losing line numbers or URL strings', () => {
  assert.deepEqual(checkGameplaySource('// page.screenshot();\n/* gl.readPixels(); */', 'flow.js'), []);
  const result = checkGameplaySource("const url = 'https://example.test';\nawait page.screenshot();", 'flow.js');
  assert.match(result[0], /^flow.js:2:/);
});

test('scans nested gameplay helpers while excluding sibling renderer files', async () => {
  const root = await mkdtemp(join(tmpdir(), 'ante-gameplay-guard-'));
  try {
    await mkdir(join(root, 'tests', 'helpers'), { recursive: true });
    await mkdir(join(root, 'renderer'));
    await writeFile(join(root, 'renderer', 'probe.js'), 'gl.readPixels();');
    await writeFile(join(root, 'tests', 'flow.spec.js'), semanticTest);
    assert.deepEqual(await checkGameplayTree(join(root, 'tests')), []);
    await writeFile(join(root, 'tests', 'helpers', 'regression.js'), 'await page.waitForTimeout(100);');
    assert.match((await checkGameplayTree(join(root, 'tests')))[0], /helpers\/regression.js:1: fixed timing wait/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
