import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import { waluau } from './index.js';

test('transforms a .walu import into an ES module', async () => {
  const root = await mkdtemp(join(tmpdir(), 'waluau-vite-plugin-'));
  try {
    const entry = join(root, 'main.walu');
    const watched = [];
    const plugin = waluau({ compiler: { command: 'true' } });
    plugin.configResolved({ root });
    const transformed = await plugin.transform.call(
      { addWatchFile: (file) => watched.push(file) },
      '',
      entry,
    );

    assert.deepEqual(watched, [entry]);
    assert.match(transformed.code, /export const game = runWaluau/);
    assert.match(transformed.code, /export default game/);
    assert.doesNotMatch(transformed.code, /virtual:waluau-game/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('takes over the viewport without injecting an entry module', () => {
  const plugin = waluau({ compiler: { command: 'true' } });
  const transformed = plugin.transformIndexHtml();
  const style = transformed.find(({ tag }) => tag === 'style');
  assert.match(style.children, /100vw !important/);
  assert.equal(transformed.some(({ tag }) => tag === 'script'), false);
});

test('allows embedding without full-screen styles', () => {
  const plugin = waluau({ fullScreen: false, compiler: { command: 'true' } });
  const transformed = plugin.transformIndexHtml();
  assert.equal(transformed.some(({ tag }) => tag === 'style'), false);
});
