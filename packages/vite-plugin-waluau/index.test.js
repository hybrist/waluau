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
    assert.match(transformed.code, /export const game = replaceWaluauGame/);
    assert.match(transformed.code, /captureWaluauGame\(game\)/);
    assert.match(transformed.code, /import\.meta\.hot\.accept\(\)/);
    assert.match(transformed.code, /waluau-hmr=1/);
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
    const key = createHash('sha256').update(entry).digest('hex').slice(0, 12);
    const report = join(root, '.waluau', key, 'report.json');
    assert.deepEqual(args.slice(-4), ['--manifest', manifest, '--report', report]);
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
    assert.deepEqual(
      await plugin.handleHotUpdate({ file: generated, modules: [], server: {} }),
      [],
      'generated module writes should not trigger a second HMR transaction',
    );
    assert.deepEqual(
      await plugin.handleHotUpdate({
        file: join(root, '.waluau', key, 'game.wasm'),
        modules: [],
        server: {},
      }),
      [],
      'generated Wasm writes should not trigger a full reload',
    );
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

test('removes the wrapped DOM listener associated with a guest callback', () => {
  const callback = {};
  let added;
  let removed;
  const target = {
    addEventListener(type, listener) {
      added = { type, listener };
    },
    removeEventListener(type, listener) {
      removed = { type, listener };
    },
  };
  const imports = buildWaluauImports(null, undefined, {
    requiredImports: [
      { module: WALUAU_IMPORT_MODULE, name: 'EventTarget.addEventListener', kind: 'function' },
      { module: WALUAU_IMPORT_MODULE, name: 'EventTarget.removeEventListener', kind: 'function' },
    ],
    bytesConstants: [],
    getWasmExports: () => ({
      __waluau_call_callback_event_unit() {},
    }),
  })[WALUAU_IMPORT_MODULE];

  imports['EventTarget.addEventListener'](target, 'keydown', callback);
  imports['EventTarget.removeEventListener'](target, 'keydown', callback);

  assert.equal(added.type, 'keydown');
  assert.equal(removed.type, 'keydown');
  assert.equal(removed.listener, added.listener);
  assert.notEqual(removed.listener, callback);
});

test('accepts development snapshot registration as a production no-op', () => {
  const imports = buildWaluauImports(null, undefined, {
    requiredImports: [
      { module: WALUAU_IMPORT_MODULE, name: '__waluau_hmr_register', kind: 'function' },
      { module: WALUAU_IMPORT_MODULE, name: '__waluau_hmr_set_snapshot', kind: 'function' },
      { module: WALUAU_IMPORT_MODULE, name: '__waluau_hmr_get_snapshot', kind: 'function' },
      { module: WALUAU_IMPORT_MODULE, name: '__waluau_hmr_set_restore_result', kind: 'function' },
    ],
    bytesConstants: [],
  })[WALUAU_IMPORT_MODULE];

  assert.doesNotThrow(() => imports.__waluau_hmr_register({}, {}, {}));
  assert.equal(imports.__waluau_hmr_get_snapshot(), '');
});

test('watches report-involved files and rebuilds only affected entries', async () => {
  const root = await mkdtemp(join(tmpdir(), 'waluau-vite-plugin-'));
  try {
    const entryA = join(root, 'a.walu');
    const entryB = join(root, 'b.walu');
    const shared = join(root, 'shared.walu');
    const counter = join(root, 'invocations.txt');
    await writeFile(counter, '');
    // Stub compiler: records which entry it was invoked for and writes a
    // build report; a.walu's graph involves shared.walu, b.walu's does not.
    const script = `
      const fs = require('node:fs');
      const args = process.argv.slice(1);
      const entry = args[0];
      fs.appendFileSync(${JSON.stringify(counter)}, entry + '\\n');
      const involved = entry.endsWith('a.walu') ? [entry, ${JSON.stringify(shared)}] : [entry];
      fs.writeFileSync(args[args.indexOf('--report') + 1], JSON.stringify({
        success: true,
        involvedFiles: involved,
        diagnostics: [],
      }));
    `;
    const plugin = waluau({ compiler: { command: process.execPath, args: ['-e', script] } });
    plugin.configResolved({ root });

    const watched = [];
    const pluginContext = { addWatchFile: (file) => watched.push(file) };
    await plugin.transform.call(pluginContext, '', entryA);
    await plugin.transform.call(pluginContext, '', entryB);
    assert(watched.includes(shared), `report-involved file should be watched: ${watched}`);

    const invocationsBefore = (await readFile(counter, 'utf8')).trim().split('\n');
    assert.equal(invocationsBefore.length, 2);

    // Editing the shared module rebuilds only the entry whose graph uses it.
    await plugin.handleHotUpdate({ file: shared, server: { ws: { send() {} } } });
    const invocations = (await readFile(counter, 'utf8')).trim().split('\n');
    assert.equal(invocations.length, 3, `unexpected rebuilds: ${invocations}`);
    assert(invocations[2].endsWith('a.walu'));

    // Editing an unrelated new file rebuilds nothing.
    await plugin.handleHotUpdate({ file: join(root, 'unrelated-not-required.walu'), server: { ws: { send() {} } } });
    const after = (await readFile(counter, 'utf8')).trim().split('\n');
    assert.equal(after.length, 3, `unrelated file should not rebuild: ${after}`);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('invalidates affected entry modules for self-accepted hot replacement', async () => {
  const root = await mkdtemp(join(tmpdir(), 'waluau-vite-plugin-'));
  try {
    const entry = join(root, 'main.walu');
    const dependency = join(root, 'game.walu');
    const script = `
      const fs = require('node:fs');
      const args = process.argv.slice(1);
      fs.writeFileSync(args[args.indexOf('--report') + 1], JSON.stringify({
        success: true,
        involvedFiles: [args[0], ${JSON.stringify(dependency)}],
        diagnostics: [],
      }));
    `;
    const plugin = waluau({
      compiler: { command: process.execPath, args: ['-e', script] },
    });
    plugin.configResolved({ root });
    await plugin.transform.call({ addWatchFile() {} }, '', entry);

    const entryModule = { id: entry };
    const invalidated = [];
    const messages = [];
    const viteServer = {
      watcher: { add() {} },
      moduleGraph: {
        getModulesByFile(file) {
          return file === entry ? new Set([entryModule]) : undefined;
        },
        invalidateModule(module) {
          invalidated.push(module);
        },
      },
      ws: { send(message) { messages.push(message); } },
    };
    plugin.configureServer(viteServer);

    const modules = await plugin.handleHotUpdate({
      file: dependency,
      modules: [],
      server: viteServer,
    });

    assert.deepEqual(modules, [entryModule]);
    assert.deepEqual(invalidated, [entryModule]);
    assert.deepEqual(messages, []);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
