import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { test } from 'node:test';

import { chromium } from 'playwright';
import { build as viteBuild, createServer as createViteServer } from 'vite';

import { waluau } from './index.js';
import { buildWaluauImports, WALUAU_IMPORT_MODULE } from './runtime.js';
import { createWaluauShaderSourceHost } from './shaders.js';

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

test('turns a .stories.walu import into a CSF module of published stories', async () => {
  const root = await mkdtemp(join(tmpdir(), 'waluau-vite-plugin-'));
  try {
    const entry = join(root, 'card.stories.walu');
    const plugin = waluau({ compiler: { command: 'true' } });
    plugin.configResolved({ root });
    const transformed = await plugin.transform.call(
      { addWatchFile: () => {} },
      `local storybook = require("waluau:engine/storybook")
local suit = storybook.select("suit", "red", {
  storybook.choice("Red", "red"),
  storybook.choice("Blue", "blue"),
})
local rank = storybook.range("rank", 13, 2, 14, 1)
storybook.publish({
    storybook.story("Face up", { draw = draw_face_up }, { suit.declaration, rank.declaration }),
    storybook.story("Face down", { draw = draw_face_down }),
})`,
      entry,
    );

    assert.match(transformed.code, /createWaluauBook\(\{/);
    assert.match(transformed.code, /export const FaceUp = \{\n  name: "Face up",/);
    assert.match(transformed.code, /export const FaceDown = \{\n  name: "Face down",/);
    assert.match(transformed.code, /args: \{"suit":0,"rank":13\}/);
    assert.match(transformed.code, /"type":"select","labels":\{"0":"Red","1":"Blue"\}/);
    assert.match(transformed.code, /"type":"range","min":2,"max":14,"step":1/);
    assert.match(
      transformed.code,
      /render: \(args\) => \(\{ book, name: "Face up", args \}\)/,
    );
    // A story module is not a game: nothing starts on import.
    assert.doesNotMatch(transformed.code, /replaceWaluauGame/);
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
    assert(args.includes('--development-dwarf'));
    const key = createHash('sha256').update(entry).digest('hex').slice(0, 12);
    const report = join(root, '.waluau', key, 'report.json');
    assert.deepEqual(args.slice(-4), ['--manifest', manifest, '--report', report]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('requests minimal exports only for production game builds', async () => {
  const root = await mkdtemp(join(tmpdir(), 'waluau-vite-plugin-'));
  try {
    const invocation = join(root, 'invocation.json');
    const script = `require('node:fs').writeFileSync(${JSON.stringify(invocation)}, JSON.stringify(process.argv.slice(1)))`;
    const context = { addWatchFile: () => {} };
    const compiledArgs = async (command, entryName) => {
      // optimize: false — the stub compiler writes no Wasm for wasm-opt to
      // consume; this test only observes the compiler invocation.
      const plugin = waluau({ optimize: false, compiler: { command: process.execPath, args: ['-e', script] } });
      plugin.configResolved({ root, command });
      await plugin.transform.call(context, '', join(root, entryName));
      return JSON.parse(await readFile(invocation, 'utf8'));
    };

    assert.ok(
      (await compiledArgs('build', 'main.walu')).includes('--minimal-exports'),
      'a production game build should prune the playground export surface',
    );
    // The dev server, vitest, and non-build entries keep the full export
    // surface: test functions and story args are reached through it.
    assert.ok(!(await compiledArgs('serve', 'main.walu')).includes('--minimal-exports'));
    assert.ok(!(await compiledArgs('build', 'main.test.walu')).includes('--minimal-exports'));
    assert.ok(!(await compiledArgs('build', 'main.stories.walu')).includes('--minimal-exports'));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('emits external DWARF in dev but leaves production builds unchanged', async () => {
  const root = await mkdtemp(join(tmpdir(), 'waluau-vite-plugin-'));
  try {
    const entry = join(root, 'main.walu');
    const devInvocation = join(root, 'dev.json');
    const buildInvocation = join(root, 'build.json');
    const compiler = invocation => ({
      command: process.execPath,
      args: ['-e', `require('node:fs').writeFileSync(${JSON.stringify(invocation)}, JSON.stringify(process.argv.slice(1)))`],
    });

    const devPlugin = waluau({ compiler: compiler(devInvocation) });
    devPlugin.configResolved({ root, command: 'serve' });
    await devPlugin.transform.call({ addWatchFile() {} }, '', entry);

    const buildPlugin = waluau({ optimize: false, compiler: compiler(buildInvocation) });
    buildPlugin.configResolved({ root, command: 'build' });
    await buildPlugin.transform.call({ addWatchFile() {} }, '', entry);

    assert(JSON.parse(await readFile(devInvocation, 'utf8')).includes('--development-dwarf'));
    assert(!JSON.parse(await readFile(buildInvocation, 'utf8')).includes('--development-dwarf'));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('serves the compiler DWARF companion and authored sources beside dev Wasm', async () => {
  const root = await mkdtemp(join(tmpdir(), 'waluau-vite-plugin-'));
  let server;
  try {
    const entry = join(root, 'main.walu');
    const dependency = join(root, 'cards', 'card#burn?100% café:wide.walu');
    await writeFile(entry, 'function main() end\n');
    await mkdir(dirname(dependency), { recursive: true });
    await writeFile(dependency, 'return function() end\n');
    const script = `
      const fs = require('node:fs');
      const args = process.argv.slice(1);
      const wasm = args[args.indexOf('-o') + 1];
      fs.writeFileSync(wasm, Buffer.from([0, 97, 115, 109, 1, 0, 0, 0]));
      fs.writeFileSync(wasm.replace(/\\.wasm$/, '.debug.wasm'), Buffer.from('external dwarf'));
      fs.writeFileSync(wasm.replace(/\\.wasm$/, '.js'), 'export const placeholder = true;');
      fs.writeFileSync(args[args.indexOf('--report') + 1], JSON.stringify({
        success: true,
        involvedFiles: [args[0], ${JSON.stringify(dependency)}],
        developmentSources: [
          { path: '__waluau/sources/files/s-main.walu', source: fs.readFileSync(args[0], 'utf8') },
          { path: '__waluau/sources/files/s-cards/s-card%23burn%3F100%25%20caf%C3%A9%3Awide.walu', source: fs.readFileSync(${JSON.stringify(dependency)}, 'utf8') },
          {
            path: '__waluau/sources/packages/s-waluau-engine/s-v1/s-graphics.walu',
            source: 'function clear() end\\n',
          },
        ],
        diagnostics: [],
      }));
    `;
    const plugin = waluau({ compiler: { command: process.execPath, args: ['-e', script] } });
    plugin.configResolved({ root, command: 'serve' });
    await plugin.transform.call({ addWatchFile() {} }, '', entry);
    server = await createViteServer({
      root,
      logLevel: 'silent',
      plugins: [plugin],
      server: { host: '127.0.0.1', port: 0 },
    });
    await server.listen();

    const key = createHash('sha256').update(entry).digest('hex').slice(0, 12);
    const origin = server.resolvedUrls.local[0];
    const cachedUrl = (path) => new URL(`/.waluau/${key}/${path}`, origin);

    const debugResponse = await fetch(cachedUrl('game.debug.wasm'));
    assert.equal(debugResponse.status, 200);
    assert.equal(await debugResponse.text(), 'external dwarf');

    const entryResponse = await fetch(cachedUrl('__waluau/sources/files/s-main.walu'));
    assert.equal(entryResponse.status, 200);
    assert.equal(await entryResponse.text(), 'function main() end\n');

    const dependencyResponse = await fetch(cachedUrl(
      '__waluau/sources/files/s-cards/s-card%23burn%3F100%25%20caf%C3%A9%3Awide.walu',
    ));
    assert.equal(dependencyResponse.status, 200);
    assert.equal(await dependencyResponse.text(), 'return function() end\n');

    const engineResponse = await fetch(cachedUrl(
      '__waluau/sources/packages/s-waluau-engine/s-v1/s-graphics.walu',
    ));
    assert.equal(engineResponse.status, 200);
    assert.equal(await engineResponse.text(), 'function clear() end\n');
  } finally {
    await server?.close();
    await rm(root, { recursive: true, force: true });
  }
});

test('rejects escaping and duplicate compiler development source paths', async () => {
  const root = await mkdtemp(join(tmpdir(), 'waluau-vite-plugin-'));
  try {
    const entry = join(root, 'main.walu');
    await writeFile(entry, 'function main() end\n');

    const compileWithSources = async (developmentSources) => {
      const script = `
        const fs = require('node:fs');
        const args = process.argv.slice(1);
        const wasm = args[args.indexOf('-o') + 1];
        fs.writeFileSync(wasm, Buffer.from([]));
        fs.writeFileSync(wasm.replace(/\\.wasm$/, '.js'), 'export const placeholder = true;');
        fs.writeFileSync(args[args.indexOf('--report') + 1], JSON.stringify({
          success: true,
          involvedFiles: [args[0]],
          developmentSources: ${JSON.stringify(developmentSources)},
          diagnostics: [],
        }));
      `;
      const plugin = waluau({ compiler: { command: process.execPath, args: ['-e', script] } });
      plugin.configResolved({ root, command: 'serve' });
      return plugin.transform.call({ addWatchFile() {} }, '', entry);
    };

    await assert.rejects(
      compileWithSources([{
        path: '__waluau/sources/files/%2E%2E/%2E%2E/game.wasm',
        source: 'escape',
      }]),
      /escapes its reserved directory/,
    );
    await assert.rejects(
      compileWithSources([
        { path: '__waluau/sources/files/s-cards%2Fs-card.walu', source: 'first' },
        { path: '__waluau/sources/files/s-cards/s-card.walu', source: 'second' },
      ]),
      /duplicate development source destination/,
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('copies filesystem DWARF sources reported by older custom compilers', async () => {
  const root = await mkdtemp(join(tmpdir(), 'waluau-vite-plugin-'));
  try {
    const entry = join(root, 'main.walu');
    const dependency = join(root, 'cards', 'card.walu');
    await mkdir(dirname(dependency), { recursive: true });
    await writeFile(entry, 'function main() end\n');
    await writeFile(dependency, 'return function() end\n');
    const script = `
      const fs = require('node:fs');
      const args = process.argv.slice(1);
      const wasm = args[args.indexOf('-o') + 1];
      fs.writeFileSync(wasm, Buffer.from([]));
      fs.writeFileSync(wasm.replace(/\\.wasm$/, '.js'), 'export const placeholder = true;');
      fs.writeFileSync(args[args.indexOf('--report') + 1], JSON.stringify({
        success: true,
        involvedFiles: [args[0], ${JSON.stringify(dependency)}],
        diagnostics: [],
      }));
    `;
    const plugin = waluau({ compiler: { command: process.execPath, args: ['-e', script] } });
    plugin.configResolved({ root, command: 'serve' });
    await plugin.transform.call({ addWatchFile() {} }, '', entry);

    const key = createHash('sha256').update(entry).digest('hex').slice(0, 12);
    const cache = join(root, '.waluau', key);
    assert.equal(await readFile(join(cache, 'main.walu'), 'utf8'), 'function main() end\n');
    assert.equal(
      await readFile(join(cache, 'cards', 'card.walu'), 'utf8'),
      'return function() end\n',
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('reuses one persistent compiler process across Vite rebuilds', async () => {
  const root = await mkdtemp(join(tmpdir(), 'waluau-vite-plugin-'));
  let plugin;
  try {
    const entry = join(root, 'main.walu');
    const serverScript = join(root, 'compiler-server.cjs');
    const starts = join(root, 'starts.txt');
    const builds = join(root, 'builds.txt');
    await writeFile(serverScript, `
      const fs = require('node:fs');
      const readline = require('node:readline');
      fs.appendFileSync(${JSON.stringify(starts)}, 'start\\n');
      const lines = readline.createInterface({ input: process.stdin });
      lines.on('line', (line) => {
        const request = JSON.parse(line);
        fs.appendFileSync(${JSON.stringify(builds)}, request.args[0] + '\\n');
        const report = request.args[request.args.indexOf('--report') + 1];
        fs.writeFileSync(report, JSON.stringify({
          success: true,
          involvedFiles: [request.args[0]],
          diagnostics: [],
        }));
        process.stdout.write(JSON.stringify({
          id: request.id,
          ok: true,
          parsesPerformed: 1,
          cachedParseCount: 1,
        }) + '\\n');
      });
    `);
    plugin = waluau({
      compiler: {
        command: process.execPath,
        args: [serverScript],
        persistent: true,
      },
    });
    plugin.configResolved({ root });
    await plugin.transform.call({ addWatchFile() {} }, '', entry);

    const entryModule = { id: entry };
    const viteServer = {
      moduleGraph: {
        getModulesByFile: () => new Set([entryModule]),
        invalidateModule() {},
      },
      ws: { send() {} },
    };
    await plugin.handleHotUpdate({ file: entry, modules: [entryModule], server: viteServer });
    const retransformed = await plugin.transform.call({ addWatchFile() {} }, '', entry);
    await plugin.closeBundle();
    plugin = null;

    assert.match(retransformed.code, /waluau-hmr=2/);
    assert.equal((await readFile(starts, 'utf8')).trim().split('\n').length, 1);
    assert.equal(
      (await readFile(builds, 'utf8')).trim().split('\n').length,
      2,
      'the post-update transform should reuse the artifact built by handleHotUpdate',
    );
  } finally {
    await plugin?.closeBundle();
    await rm(root, { recursive: true, force: true });
  }
});

test('loads shader sources in production and accepts dev updates without rebuilding Wasm', async () => {
  const root = await mkdtemp(join(tmpdir(), 'waluau-vite-plugin-'));
  try {
    const entry = join(root, 'main.walu');
    const vertex = join(root, 'shaders', 'effect.vert');
    // Deliberately use .walu and include it in the compiler report. Configured
    // source classification must still win over generic Waluau rebuild logic.
    const pixel = join(root, 'shaders', 'effect.walu');
    const counter = join(root, 'compiler-invocations.txt');
    await writeFile(counter, '');
    const script = `
      const fs = require('node:fs');
      const args = process.argv.slice(1);
      fs.appendFileSync(${JSON.stringify(counter)}, args[0] + '\\n');
      fs.writeFileSync(args[args.indexOf('--report') + 1], JSON.stringify({
        success: true,
        involvedFiles: [args[0], ${JSON.stringify(pixel)}],
        diagnostics: [],
      }));
    `;
    const plugin = waluau({
      shaderSources: {
        'effect.vertex': 'shaders/effect.vert',
        'effect.pixel': 'shaders/effect.walu',
        ['__proto__']: 'shaders/effect.vert',
      },
      compiler: { command: process.execPath, args: ['-e', script] },
    });
    plugin.configResolved({ root });
    const transformed = await plugin.transform.call({ addWatchFile() {} }, '', entry);
    const shaderModuleId = /import shaderSourceHost from "(virtual:waluau-shader-sources:[^"]+)"/
      .exec(transformed.code)?.[1];
    assert(shaderModuleId, 'transformed game should import its isolated shader source module');
    const shaderModule = plugin.load(plugin.resolveId(shaderModuleId));

    // These eager raw imports are present in both Vite dev and production
    // through an isolated module; production bundles the same initial text.
    assert.match(shaderModule, new RegExp(
      `import waluauShaderSource0 from ${JSON.stringify(`${vertex}?raw`).replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}`,
    ));
    assert.match(shaderModule, new RegExp(
      `import waluauShaderSource1 from ${JSON.stringify(`${pixel}?raw`).replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}`,
    ));
    assert.match(shaderModule, /\["effect\.vertex"\]: waluauShaderSource0/);
    assert.match(shaderModule, /\["__proto__"\]: waluauShaderSource2/);
    assert.match(transformed.code, /shaderSources: shaderSourceHost/);

    // Every dependency owns its key-specific accept callback. Updating just
    // the pixel module cannot shift its module into the vertex key.
    assert.match(
      shaderModule,
      /accept\([^)]*effect\.vert\?raw[^]*shaderSourceHost\.update\("effect\.vertex", module\?\.default\)/,
    );
    assert.match(
      shaderModule,
      /accept\([^)]*effect\.walu\?raw[^]*shaderSourceHost\.update\("effect\.pixel", module\?\.default\)/,
    );

    const invalidated = [];
    const messages = [];
    const result = await plugin.handleHotUpdate({
      file: pixel,
      modules: [{ id: `${pixel}?raw` }],
      server: {
        moduleGraph: {
          getModulesByFile() {
            throw new Error('a shader edit must not inspect Waluau entry modules');
          },
          invalidateModule(module) {
            invalidated.push(module);
          },
        },
        ws: { send(message) { messages.push(message); } },
      },
    });

    assert.equal(result, undefined);
    assert.equal((await readFile(counter, 'utf8')).trim().split('\n').length, 1);
    assert.deepEqual(invalidated, []);
    assert.deepEqual(messages, []);
    assert.doesNotMatch(
      shaderModule.match(/accept\([^)]*effect\.walu\?raw[^]*?\n  \}\);/)?.[0] ?? '',
      /runWaluau|replaceWaluauGame|invalidate/,
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('bundles configured shader source text in a production Vite build', async () => {
  // Vite 8's HTML emitter requires the temporary root to be below the build
  // process cwd when using the in-memory write:false output.
  const root = await mkdtemp(join(resolve('.'), '.waluau-vite-plugin-build-'));
  try {
    const shaderSentinel = 'PRODUCTION_SHADER_SOURCE_SENTINEL_9fc7';
    const compiler = join(root, 'compiler.cjs');
    await mkdir(join(root, 'shaders'), { recursive: true });
    await writeFile(
      join(root, 'index.html'),
      '<script type="module" src="/main.walu"></script>',
    );
    await writeFile(join(root, 'main.walu'), 'print("production build")');
    await writeFile(join(root, 'shaders', 'effect.frag'), shaderSentinel);
    await writeFile(compiler, `
      const fs = require('node:fs');
      const path = require('node:path');
      const args = process.argv.slice(2);
      const wasm = args[args.indexOf('-o') + 1];
      const report = args[args.indexOf('--report') + 1];
      fs.mkdirSync(path.dirname(wasm), { recursive: true });
      fs.writeFileSync(wasm, Buffer.from([]));
      fs.writeFileSync(path.join(path.dirname(wasm), 'game.js'), [
        'export const wasmUrl = null;',
        'export async function run() { return { exports: {} }; }',
      ].join('\\n'));
      fs.writeFileSync(report, JSON.stringify({
        success: true,
        involvedFiles: [args[0]],
        diagnostics: [],
      }));
    `);

    const output = await viteBuild({
      root,
      configFile: false,
      logLevel: 'silent',
      resolve: {
        alias: [
          {
            find: '@waluau/vite-plugin/runtime',
            replacement: resolve('packages/vite-plugin-waluau/runtime.js'),
          },
          {
            find: '@waluau/vite-plugin/hot',
            replacement: resolve('packages/vite-plugin-waluau/hot.js'),
          },
          {
            find: '@waluau/vite-plugin/shaders',
            replacement: resolve('packages/vite-plugin-waluau/shaders.js'),
          },
        ],
      },
      plugins: [waluau({
        fullScreen: false,
        shaderSources: { pixel: 'shaders/effect.frag' },
        compiler: { command: process.execPath, args: [compiler] },
      })],
      build: {
        minify: false,
        write: false,
      },
    });
    const outputs = Array.isArray(output) ? output.flatMap((result) => result.output) : output.output;
    const bundledCode = outputs
      .filter((item) => item.type === 'chunk')
      .map((item) => item.code)
      .join('\n');

    assert.match(bundledCode, new RegExp(shaderSentinel));
    assert.doesNotMatch(bundledCode, /import\.meta\.hot/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('delivers a real Vite shader HMR update without replacing the running game', async () => {
  const root = await mkdtemp(join(resolve('.'), '.waluau-vite-plugin-hmr-'));
  let server;
  let browser;
  try {
    const shader = join(root, 'shaders', 'effect.frag');
    const compiler = join(root, 'compiler.cjs');
    const counter = join(root, 'compiler-invocations.txt');
    await mkdir(dirname(shader), { recursive: true });
    await writeFile(
      join(root, 'index.html'),
      '<script type="module" src="/main.walu"></script>',
    );
    await writeFile(join(root, 'main.walu'), 'print("development hmr")');
    await writeFile(shader, 'initial live shader');
    await writeFile(counter, '');
    await writeFile(compiler, `
      const fs = require('node:fs');
      const path = require('node:path');
      const args = process.argv.slice(2);
      const wasm = args[args.indexOf('-o') + 1];
      const report = args[args.indexOf('--report') + 1];
      fs.appendFileSync(${JSON.stringify(counter)}, 'compile\\n');
      fs.mkdirSync(path.dirname(wasm), { recursive: true });
      fs.writeFileSync(wasm, Buffer.from([]));
      fs.writeFileSync(path.join(path.dirname(wasm), 'game.js'), [
        'export const wasmUrl = null;',
        'export async function run({ createImports }) {',
        '  const exports = {};',
        '  const imports = createImports({',
        '    requiredImports: [], bytesConstants: [],',
        '    getWasmExports: () => exports,',
        '    assetBaseUrl: new URL("./", import.meta.url), assetManifest: {},',
        '  }).waluau;',
        '  const marker = {};',
        '  globalThis.__waluauShaderRuns = (globalThis.__waluauShaderRuns || 0) + 1;',
        '  globalThis.__waluauShaderTest = {',
        '    marker,',
        '    revision: () => imports.__waluau_shader_source_revision("pixel"),',
        '    text: () => imports.__waluau_shader_source_text("pixel"),',
        '  };',
        '  return { exports };',
        '}',
      ].join('\\n'));
      fs.writeFileSync(report, JSON.stringify({
        success: true, involvedFiles: [args[0]], diagnostics: [],
      }));
    `);

    server = await createViteServer({
      root,
      configFile: false,
      logLevel: 'silent',
      resolve: {
        alias: [
          {
            find: '@waluau/vite-plugin/runtime',
            replacement: resolve('packages/vite-plugin-waluau/runtime.js'),
          },
          {
            find: '@waluau/vite-plugin/hot',
            replacement: resolve('packages/vite-plugin-waluau/hot.js'),
          },
          {
            find: '@waluau/vite-plugin/shaders',
            replacement: resolve('packages/vite-plugin-waluau/shaders.js'),
          },
        ],
      },
      plugins: [waluau({
        fullScreen: false,
        shaderSources: { pixel: 'shaders/effect.frag' },
        compiler: { command: process.execPath, args: [compiler] },
      })],
      server: { host: '127.0.0.1', port: 0 },
    });
    await server.listen();

    browser = await chromium.launch({ headless: true });
    const page = await browser.newPage();
    let loads = 0;
    page.on('load', () => { loads += 1; });
    await page.goto(server.resolvedUrls.local[0]);
    await page.waitForFunction(() => globalThis.__waluauShaderTest?.revision() === 1);
    await page.evaluate(() => {
      globalThis.__waluauInitialMarker = globalThis.__waluauShaderTest.marker;
    });
    const initialLoads = loads;

    await writeFile(shader, 'updated live shader');
    await page.waitForFunction(
      () => (
        globalThis.__waluauShaderTest?.revision() === 2
        && globalThis.__waluauShaderTest?.text() === 'updated live shader'
      ),
    );

    assert.equal(await page.evaluate(() => globalThis.__waluauShaderRuns), 1);
    assert.equal(
      await page.evaluate(
        () => globalThis.__waluauShaderTest.marker === globalThis.__waluauInitialMarker,
      ),
      true,
    );
    assert.equal(loads, initialLoads);
    assert.equal((await readFile(counter, 'utf8')).trim().split('\n').length, 1);
  } finally {
    await browser?.close();
    await server?.close();
    await rm(root, { recursive: true, force: true });
  }
});

// A module whose only content is one private, unused function. wasm-opt -Oz
// deletes the function, so the optimized module is exactly the 8-byte header.
const unoptimizedWasm = [
  0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // \0asm, version 1
  0x01, 0x04, 0x01, 0x60, 0x00, 0x00, // type section: () -> ()
  0x03, 0x02, 0x01, 0x00, // function section: one function of type 0
  0x0a, 0x04, 0x01, 0x02, 0x00, 0x0b, // code section: empty body
];

function wasmWritingCompiler() {
  const script = `
    const fs = require('node:fs');
    const args = process.argv.slice(1);
    fs.writeFileSync(args[args.indexOf('-o') + 1], Buffer.from(${JSON.stringify(unoptimizedWasm)}));
    fs.writeFileSync(args[args.indexOf('--report') + 1], JSON.stringify({
      success: true,
      involvedFiles: [args[0]],
      diagnostics: [],
    }));
  `;
  return { command: process.execPath, args: ['-e', script] };
}

async function compiledWasm(root, entry) {
  const key = createHash('sha256').update(entry).digest('hex').slice(0, 12);
  return readFile(join(root, '.waluau', key, 'game.wasm'));
}

test('optimizes compiled Wasm with wasm-opt in production builds only', async () => {
  const root = await mkdtemp(join(tmpdir(), 'waluau-vite-plugin-'));
  try {
    const entry = join(root, 'main.walu');

    const devPlugin = waluau({ compiler: wasmWritingCompiler() });
    devPlugin.configResolved({ root, command: 'serve' });
    await devPlugin.transform.call({ addWatchFile() {} }, '', entry);
    assert.deepEqual(
      Array.from(await compiledWasm(root, entry)),
      unoptimizedWasm,
      'the dev server should serve the compiler output untouched',
    );

    const buildPlugin = waluau({ compiler: wasmWritingCompiler() });
    buildPlugin.configResolved({ root, command: 'build' });
    await buildPlugin.transform.call({ addWatchFile() {} }, '', entry);
    const optimized = await compiledWasm(root, entry);
    assert.deepEqual(
      Array.from(optimized),
      unoptimizedWasm.slice(0, 8),
      'wasm-opt should have removed the unused function',
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('leaves production Wasm untouched when optimize is disabled', async () => {
  const root = await mkdtemp(join(tmpdir(), 'waluau-vite-plugin-'));
  try {
    const entry = join(root, 'main.walu');
    const plugin = waluau({ optimize: false, compiler: wasmWritingCompiler() });
    plugin.configResolved({ root, command: 'build' });
    await plugin.transform.call({ addWatchFile() {} }, '', entry);
    assert.deepEqual(Array.from(await compiledWasm(root, entry)), unoptimizedWasm);
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

test('bridges configured shader sources and makes absent hosts recoverable', () => {
  const requiredImports = [
    { module: WALUAU_IMPORT_MODULE, name: '__waluau_shader_source_revision', kind: 'function' },
    { module: WALUAU_IMPORT_MODULE, name: '__waluau_shader_source_text', kind: 'function' },
  ];
  const absent = buildWaluauImports(null, undefined, {
    requiredImports,
    bytesConstants: [],
  })[WALUAU_IMPORT_MODULE];

  assert.equal(absent.__waluau_shader_source_revision('pixel'), -1);
  assert.equal(absent.__waluau_shader_source_text('pixel'), '');

  const shaderSources = createWaluauShaderSourceHost({ pixel: 'bundled pixel' });
  const configured = buildWaluauImports(null, undefined, {
    requiredImports,
    bytesConstants: [],
    shaderSources,
  })[WALUAU_IMPORT_MODULE];
  assert.equal(configured.__waluau_shader_source_revision('pixel'), 1);
  assert.equal(configured.__waluau_shader_source_text('pixel'), 'bundled pixel');
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
