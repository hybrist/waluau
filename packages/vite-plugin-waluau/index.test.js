import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
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

test('passes a resolved asset manifest to the compiler', async () => {
  const root = await mkdtemp(join(tmpdir(), 'waluau-vite-plugin-'));
  try {
    const entry = join(root, 'main.walu');
    const manifest = join(root, 'waluau.assets.json');
    const asset = join(root, 'assets', 'card.svg');
    const invocation = join(root, 'invocation.json');
    const script = `require('node:fs').writeFileSync(${JSON.stringify(invocation)}, JSON.stringify(process.argv.slice(1)))`;
    await writeFile(manifest, JSON.stringify({
      version: 1,
      assets: [{ path: 'assets/card.svg', type: 'image' }],
    }));
    const watched = [];
    const plugin = waluau({
      manifest: 'waluau.assets.json',
      compiler: { command: process.execPath, args: ['-e', script] },
    });
    plugin.configResolved({ root });

    await plugin.transform.call(
      { addWatchFile: (file) => watched.push(file) },
      '',
      entry,
    );

    assert.deepEqual(watched, [entry, manifest, asset]);
    const args = JSON.parse(await readFile(invocation, 'utf8'));
    assert.deepEqual(args.slice(-2), ['--manifest', manifest]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('turns generated manifest URLs into Vite asset imports', async () => {
  const root = await mkdtemp(join(tmpdir(), 'waluau-vite-plugin-'));
  try {
    const entry = join(root, 'main.walu');
    const plugin = waluau({ compiler: { command: 'true' } });
    plugin.configResolved({ root });
    await plugin.transform.call({ addWatchFile() {} }, '', entry);

    const key = createHash('sha256').update(entry).digest('hex').slice(0, 12);
    const generated = join(root, '.waluau', key, 'game.js');
    const transformed = await plugin.transform(
      'export const assetManifest = Object.freeze({\n'
        + '  "assets/card.svg": Object.freeze({ url: "./assets/card.hash.svg", type: "image" }),\n'
        + '});\n',
      generated,
    );

    assert.match(transformed, /import waluauAssetUrl0 from "\.\/assets\/card\.hash\.svg\?url";/);
    assert.match(transformed, /url: waluauAssetUrl0/);
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
