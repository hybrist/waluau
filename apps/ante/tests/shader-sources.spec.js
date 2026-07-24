import { test, expect } from '@playwright/test';
import {
  cp,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rm,
  stat,
  writeFile,
} from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { createServer as createViteServer } from 'vite';

const GAME_READY_TIMEOUT = 20_000;
const ANTE_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const VITE_CONFIG = join(ANTE_ROOT, 'vite.config.js');

const SHADER_SOURCES = Object.freeze({
  'ante.effects.vertex': 'src/shaders/effects.vert',
  'ante.effects.gold-shimmer': 'src/shaders/gold-shimmer.frag',
  'ante.effects.defeat-shroud': 'src/shaders/defeat-shroud.frag',
  'ante.effects.card-burn': 'src/shaders/card-burn.frag',
  'ante.effects.red-fire': 'src/shaders/red-fire.frag',
  'ante.effects.blue-caustics': 'src/shaders/blue-caustics.frag',
  'ante.effects.green-growth': 'src/shaders/green-growth.frag',
  'ante.effects.black-hole': 'src/shaders/black-hole.frag',
});

const VERTEX_NAME = 'ante.effects.vertex';
const DEFEAT_NAME = 'ante.effects.defeat-shroud';
const PIXEL_NAMES = Object.keys(SHADER_SOURCES).filter((name) => name !== VERTEX_NAME);

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

async function readShaderSources(root = ANTE_ROOT) {
  return Object.fromEntries(await Promise.all(
    Object.entries(SHADER_SOURCES).map(async ([name, path]) => [
      name,
      await readFile(join(root, path), 'utf8'),
    ]),
  ));
}

async function installRuntimeProbe(page) {
  await page.addInitScript(() => {
    const shaderText = new WeakMap();
    const attachedShaders = new WeakMap();
    const deletedPrograms = new WeakSet();
    const programs = [];
    const sources = [];

    const prototype = WebGL2RenderingContext.prototype;
    const originalShaderSource = prototype.shaderSource;
    const originalCreateProgram = prototype.createProgram;
    const originalAttachShader = prototype.attachShader;
    const originalLinkProgram = prototype.linkProgram;
    const originalDeleteProgram = prototype.deleteProgram;

    let programCreates = 0;
    let programDeletes = 0;
    let wasmInstantiations = 0;

    prototype.shaderSource = function shaderSource(shader, source) {
      const text = String(source);
      shaderText.set(shader, text);
      sources.push(text);
      return originalShaderSource.call(this, shader, source);
    };
    prototype.createProgram = function createProgram() {
      const program = originalCreateProgram.call(this);
      programCreates += 1;
      attachedShaders.set(program, []);
      return program;
    };
    prototype.attachShader = function attachShader(program, shader) {
      attachedShaders.get(program)?.push(shader);
      return originalAttachShader.call(this, program, shader);
    };
    prototype.linkProgram = function linkProgram(program) {
      const result = originalLinkProgram.call(this, program);
      programs.push({
        program,
        sources: (attachedShaders.get(program) ?? []).map((shader) => shaderText.get(shader)),
      });
      return result;
    };
    prototype.deleteProgram = function deleteProgram(program) {
      programDeletes += 1;
      deletedPrograms.add(program);
      return originalDeleteProgram.call(this, program);
    };

    const originalInstantiate = WebAssembly.instantiate;
    const originalInstantiateStreaming = WebAssembly.instantiateStreaming;
    WebAssembly.instantiate = function instantiate(...args) {
      wasmInstantiations += 1;
      return Reflect.apply(originalInstantiate, this, args);
    };
    WebAssembly.instantiateStreaming = function instantiateStreaming(...args) {
      wasmInstantiations += 1;
      return Reflect.apply(originalInstantiateStreaming, this, args);
    };

    globalThis.__anteShaderProbe = {
      sources,
      programs,
      deletedPrograms,
      snapshot() {
        return {
          programCreates,
          programDeletes,
          wasmInstantiations,
        };
      },
    };
  });
}

async function openGame(page, url = '/') {
  await page.goto(url);
  await expect(page.locator('h1')).toHaveText('Arcane Heist', {
    timeout: GAME_READY_TIMEOUT,
  });
  return page.locator('canvas#walua-game-canvas');
}

function sourceCounts(page, shaderSources) {
  return page.evaluate((expected) => Object.fromEntries(
    Object.entries(expected).map(([name, source]) => [
      name,
      globalThis.__anteShaderProbe.sources.filter((candidate) => candidate === source).length,
    ]),
  ), shaderSources);
}

function probeSnapshot(page) {
  return page.evaluate(() => globalThis.__anteShaderProbe.snapshot());
}

async function waitForRuntimeToSettle(page) {
  let previous = '';
  let stableSamples = 0;
  for (let attempt = 0; attempt < 80; attempt += 1) {
    const current = await page.evaluate(() => JSON.stringify({
      sourceCount: globalThis.__anteShaderProbe.sources.length,
      ...globalThis.__anteShaderProbe.snapshot(),
    }));
    if (current === previous) stableSamples += 1;
    else stableSamples = 0;
    if (stableSamples === 4) return;
    previous = current;
    await page.waitForTimeout(100);
  }
  throw new Error('Ante development runtime did not settle');
}

function liveProgramCount(page, source) {
  return page.evaluate((expectedSource) => {
    const probe = globalThis.__anteShaderProbe;
    return probe.programs.filter(
      ({ program, sources }) => (
        sources.includes(expectedSource) && !probe.deletedPrograms.has(program)
      ),
    ).length;
  }, source);
}

function magentaPixelCount(canvas) {
  return canvas.evaluate((node) => {
    const gl = node.getContext('webgl2');
    const pixels = new Uint8Array(node.width * node.height * 4);
    gl.readPixels(0, 0, node.width, node.height, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
    let count = 0;
    for (let index = 0; index < pixels.length; index += 4) {
      if (
        pixels[index] > 240
        && pixels[index + 1] < 40
        && pixels[index + 2] > 240
        && pixels[index + 3] > 200
      ) {
        count += 1;
      }
    }
    return count;
  });
}

async function findFile(root, basename) {
  const entries = await readdir(root, { withFileTypes: true });
  for (const entry of entries) {
    const path = join(root, entry.name);
    if (entry.isDirectory()) {
      const nested = await findFile(path, basename);
      if (nested != null) return nested;
    } else if (entry.name === basename) {
      return path;
    }
  }
  return null;
}

async function copyAnteApp(destination) {
  await Promise.all([
    cp(join(ANTE_ROOT, 'index.html'), join(destination, 'index.html')),
    cp(join(ANTE_ROOT, 'waluau.assets.json'), join(destination, 'waluau.assets.json')),
    cp(join(ANTE_ROOT, 'src'), join(destination, 'src'), { recursive: true }),
    cp(join(ANTE_ROOT, 'assets'), join(destination, 'assets'), { recursive: true }),
  ]);
}

test('production includes and compiles all eight external shader sources', async ({ page }) => {
  const shaders = await readShaderSources();
  const configSource = await readFile(VITE_CONFIG, 'utf8');
  const renderSource = await readFile(join(ANTE_ROOT, 'src', 'render.walu'), 'utf8');

  expect(Object.keys(shaders)).toEqual([
    VERTEX_NAME,
    ...PIXEL_NAMES,
  ]);
  for (const [name, path] of Object.entries(SHADER_SOURCES)) {
    expect(configSource).toMatch(new RegExp(
      `['"]${escapeRegExp(name)}['"]\\s*:\\s*['"]${escapeRegExp(path)}['"]`,
    ));
    expect(renderSource).not.toContain(shaders[name].trim());
  }

  await installRuntimeProbe(page);
  await openGame(page);

  const expectedCounts = Object.fromEntries([
    // The engine's default renderer uses the same vertex contract once, then
    // Ante compiles it once for each of its seven external effect programs.
    [VERTEX_NAME, 8],
    ...PIXEL_NAMES.map((name) => [name, 1]),
  ]);
  await expect.poll(
    () => sourceCounts(page, shaders),
    { timeout: GAME_READY_TIMEOUT },
  ).toEqual(expectedCounts);

  await page.waitForTimeout(250);
  expect(await sourceCounts(page, shaders)).toEqual(expectedCounts);
  await expect(page.locator('canvas#walua-game-canvas')).toBeVisible();
});

test('development HMR fans out, retains the last good shader, and recovers', async ({ page }) => {
  test.slow();
  await installRuntimeProbe(page);

  const cacheRoot = join(ANTE_ROOT, '.waluau');
  await mkdir(cacheRoot, { recursive: true });
  const temporaryRoot = await mkdtemp(join(cacheRoot, 'shader-hmr-'));
  let server;
  try {
    await copyAnteApp(temporaryRoot);
    const shaders = await readShaderSources(temporaryRoot);
    const defeatPath = join(temporaryRoot, SHADER_SOURCES[DEFEAT_NAME]);
    const vertexPath = join(temporaryRoot, SHADER_SOURCES[VERTEX_NAME]);
    const initialDefeatSource = shaders[DEFEAT_NAME];
    const initialVertexSource = shaders[VERTEX_NAME];

    server = await createViteServer({
      root: temporaryRoot,
      configFile: VITE_CONFIG,
      logLevel: 'silent',
      server: {
        host: '127.0.0.1',
        port: 0,
      },
    });
    await server.listen();

    let loadCount = 0;
    let wasmRequestCount = 0;
    const pageErrors = [];
    const consoleMessages = [];
    page.on('load', () => { loadCount += 1; });
    page.on('request', (request) => {
      if (/\.wasm(?:\?|$)/.test(request.url())) wasmRequestCount += 1;
    });
    page.on('pageerror', (error) => pageErrors.push(error.message));
    page.on('console', (message) => consoleMessages.push(message.text()));

    await openGame(page, server.resolvedUrls.local[0]);
    // Vite can finish compiler-generated module updates immediately after the
    // first page request. Let those normal startup transactions become quiet
    // before taking the no-replacement HMR baseline. Superseded transactions
    // can leave an inert pending root because hot disposal intentionally
    // suspends rather than removes the canvas; they are irrelevant to the
    // mounted game this test observes.
    await waitForRuntimeToSettle(page);
    await page.evaluate(() => {
      for (const root of document.querySelectorAll('main#walua-game-pending')) {
        root.remove();
      }
    });
    const mountedCanvas = page.locator('main#walua-game canvas#walua-game-canvas');
    const initialCounts = await sourceCounts(page, shaders);
    expect(initialCounts[VERTEX_NAME]).toBeGreaterThan(0);
    for (const name of PIXEL_NAMES) expect(initialCounts[name]).toBeGreaterThan(0);

    const wasmPath = await findFile(join(temporaryRoot, '.waluau'), 'game.wasm');
    expect(wasmPath).not.toBeNull();
    const initialWasmMtime = (await stat(wasmPath, { bigint: true })).mtimeNs;
    const initialProbe = await probeSnapshot(page);
    const initialLoadCount = loadCount;
    const initialWasmRequestCount = wasmRequestCount;
    const baselineMagentaPixels = await magentaPixelCount(mountedCanvas);
    await page.evaluate(() => {
      globalThis.__anteInitialRoot = document.querySelector('main#walua-game');
    });

    const fragmentUpdates = {};
    let afterFragments = initialProbe;
    for (const name of PIXEL_NAMES) {
      expect(await liveProgramCount(page, shaders[name])).toBe(1);
      const update = `${shaders[name]}\n// ante-hmr-${name}-update\n`;
      await writeFile(join(temporaryRoot, SHADER_SOURCES[name]), update);
      await expect.poll(
        () => sourceCounts(page, { [name]: update }),
        { timeout: GAME_READY_TIMEOUT },
      ).toEqual({ [name]: 1 });
      await expect.poll(probeSnapshot.bind(null, page)).toMatchObject({
        programCreates: afterFragments.programCreates + 1,
        programDeletes: afterFragments.programDeletes + 1,
      });
      expect(await liveProgramCount(page, shaders[name])).toBe(0);
      expect(await liveProgramCount(page, update)).toBe(1);
      await page.waitForTimeout(150);
      expect(await sourceCounts(page, { [name]: update })).toEqual({ [name]: 1 });
      fragmentUpdates[name] = update;
      afterFragments = await probeSnapshot(page);
    }

    // Optional effects warn without taking over the canvas, retain their
    // previous program, and recover on the next valid revision.
    const goldName = 'ante.effects.gold-shimmer';
    const validGold = fragmentUpdates[goldName];
    const beforeOptionalInvalid = await probeSnapshot(page);
    const invalidGold = `${validGold}\nANTE_HMR_INVALID_OPTIONAL\n`;
    await writeFile(join(temporaryRoot, SHADER_SOURCES[goldName]), invalidGold);
    await expect.poll(
      () => sourceCounts(page, { optionalInvalid: invalidGold }),
      { timeout: GAME_READY_TIMEOUT },
    ).toEqual({ optionalInvalid: 1 });
    await expect.poll(
      () => consoleMessages.some(
        (message) => message.includes('ante: optional shader warning: gold shimmer'),
      ),
      { timeout: GAME_READY_TIMEOUT },
    ).toBe(true);
    expect(await probeSnapshot(page)).toMatchObject(beforeOptionalInvalid);
    expect(await liveProgramCount(page, validGold)).toBe(1);
    expect(await magentaPixelCount(mountedCanvas)).toBeLessThan(
      baselineMagentaPixels + 500,
    );

    const repairedGold = `${shaders[goldName]}\n// ante-hmr-optional-repaired\n`;
    await writeFile(join(temporaryRoot, SHADER_SOURCES[goldName]), repairedGold);
    await expect.poll(
      () => sourceCounts(page, { optionalRepaired: repairedGold }),
      { timeout: GAME_READY_TIMEOUT },
    ).toEqual({ optionalRepaired: 1 });
    await expect.poll(probeSnapshot.bind(null, page)).toMatchObject({
      programCreates: beforeOptionalInvalid.programCreates + 1,
      programDeletes: beforeOptionalInvalid.programDeletes + 1,
    });
    fragmentUpdates[goldName] = repairedGold;
    afterFragments = await probeSnapshot(page);

    const vertexUpdate = `${initialVertexSource}\n// ante-hmr-vertex-update\n`;
    await writeFile(vertexPath, vertexUpdate);
    await expect.poll(
      () => sourceCounts(page, { vertex: vertexUpdate }),
      { timeout: GAME_READY_TIMEOUT },
    ).toEqual({ vertex: 7 });
    await expect.poll(probeSnapshot.bind(null, page)).toMatchObject({
      programCreates: afterFragments.programCreates + 7,
      programDeletes: afterFragments.programDeletes + 7,
    });
    await page.waitForTimeout(250);
    expect(await sourceCounts(page, { vertex: vertexUpdate })).toEqual({ vertex: 7 });

    const beforeInvalid = await probeSnapshot(page);
    const validDefeat = fragmentUpdates[DEFEAT_NAME];
    expect(await liveProgramCount(page, validDefeat)).toBe(1);
    const invalidDefeat = `${validDefeat}\nANTE_HMR_INVALID_DEFEAT\n`;
    await writeFile(defeatPath, invalidDefeat);
    await expect.poll(
      () => sourceCounts(page, { invalid: invalidDefeat }),
      { timeout: GAME_READY_TIMEOUT },
    ).toEqual({ invalid: 1 });
    await expect.poll(
      () => magentaPixelCount(mountedCanvas),
      { timeout: GAME_READY_TIMEOUT },
    ).toBeGreaterThan(baselineMagentaPixels + 1_000);
    await page.waitForTimeout(250);
    expect(await sourceCounts(page, { invalid: invalidDefeat })).toEqual({ invalid: 1 });
    expect(await probeSnapshot(page)).toMatchObject(beforeInvalid);
    expect(await liveProgramCount(page, validDefeat)).toBe(1);

    const repairedDefeat = `${initialDefeatSource}\n// ante-hmr-fragment-repaired\n`;
    await writeFile(defeatPath, repairedDefeat);
    await expect.poll(
      () => sourceCounts(page, { repaired: repairedDefeat }),
      { timeout: GAME_READY_TIMEOUT },
    ).toEqual({ repaired: 1 });
    await expect.poll(probeSnapshot.bind(null, page)).toMatchObject({
      programCreates: beforeInvalid.programCreates + 1,
      programDeletes: beforeInvalid.programDeletes + 1,
    });
    await expect.poll(
      () => magentaPixelCount(mountedCanvas),
      { timeout: GAME_READY_TIMEOUT },
    ).toBeLessThan(baselineMagentaPixels + 500);
    expect(await liveProgramCount(page, validDefeat)).toBe(0);
    expect(await liveProgramCount(page, repairedDefeat)).toBe(1);

    await page.waitForTimeout(250);
    const finalProbe = await probeSnapshot(page);
    expect(await sourceCounts(page, { repaired: repairedDefeat })).toEqual({ repaired: 1 });
    expect(finalProbe).toMatchObject({
      programCreates: beforeInvalid.programCreates + 1,
      programDeletes: beforeInvalid.programDeletes + 1,
    });
    expect(finalProbe.wasmInstantiations).toBe(initialProbe.wasmInstantiations);
    expect(loadCount).toBe(initialLoadCount);
    expect(wasmRequestCount).toBe(initialWasmRequestCount);
    expect((await stat(wasmPath, { bigint: true })).mtimeNs).toBe(initialWasmMtime);
    expect(await page.evaluate(
      () => document.querySelector('main#walua-game') === globalThis.__anteInitialRoot,
    )).toBe(true);
    expect(pageErrors).toEqual([]);
  } finally {
    await server?.close();
    await rm(temporaryRoot, { recursive: true, force: true });
  }
});
