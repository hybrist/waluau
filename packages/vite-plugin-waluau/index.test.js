import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import { waluau } from './index.js';
import { buildWaluauImports, WALUAU_IMPORT_MODULE } from './runtime.js';

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

test('formats exponential numbers with Lua exponent width and case', () => {
  const imports = buildWaluauImports(null, undefined, {
    requiredImports: [
      { module: WALUAU_IMPORT_MODULE, name: 'string_format2', kind: 'function' },
    ],
    bytesConstants: [],
  });

  assert.equal(
    imports[WALUAU_IMPORT_MODULE].string_format2('%e %E', 1.5, -1.5),
    '1.500000e+00 -1.500000E+00',
  );
});

test('quotes strings as Lua source literals', () => {
  const imports = buildWaluauImports(null, undefined, {
    requiredImports: [
      { module: WALUAU_IMPORT_MODULE, name: 'string_format1', kind: 'function' },
    ],
    bytesConstants: [],
  });
  const format = imports[WALUAU_IMPORT_MODULE].string_format1;

  assert.equal(format('%q', '"ílo"\n\\'), '"\\"ílo\\"\\\n\\\\"');
  assert.equal(format('%q', '\0'), '"\\000"');
  assert.equal(format('%q', '\r'), '"\\r"');
});
