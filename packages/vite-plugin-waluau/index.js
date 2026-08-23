import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, readFileSync } from 'node:fs';
import { mkdir, rename } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { dirname, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

import { createCompilerHost } from './compiler-host.js';
import { parseStories } from './stories.js';

const packageRoot = dirname(fileURLToPath(import.meta.url));
const repositoryRoot = resolve(packageRoot, '../..');
const shaderModulePrefix = 'virtual:waluau-shader-sources:';

function run(command, args, cwd) {
  return new Promise((resolveRun, rejectRun) => {
    const child = spawn(command, args, { cwd, stdio: 'inherit' });
    child.on('error', rejectRun);
    child.on('exit', (code) => {
      if (code === 0) {
        resolveRun();
      } else {
        rejectRun(new Error(`${command} ${args.join(' ')} failed with exit code ${code}`));
      }
    });
  });
}

// wasm-opt must not run with `-all`: binaryen 132 then enables post-MVP
// features (shared-everything threads among them) and produces a module V8
// rejects with "unknown import kind 0x7e". Enable exactly the features the
// compiler emits; exception handling is required because Waluau throws with
// a Wasm tag.
const wasmOptArgs = [
  '--enable-gc',
  '--enable-reference-types',
  '--enable-bulk-memory',
  '--enable-nontrapping-float-to-int',
  '--enable-sign-ext',
  '--enable-mutable-globals',
  '--enable-multivalue',
  '--enable-exception-handling',
  '-Oz',
];

// The binaryen package ships wasm-opt as a Node script; resolve it from the
// package's own location rather than assuming a node_modules/.bin layout.
function wasmOptCommand() {
  const require = createRequire(import.meta.url);
  const binaryenRoot = dirname(require.resolve('binaryen/package.json'));
  return resolve(binaryenRoot, 'bin', 'wasm-opt');
}

async function optimizeWasm(wasmPath) {
  // The emitted game.js references the Wasm by its original name, so the
  // optimized module replaces it in place via a sibling temp file.
  const optimizedPath = `${wasmPath}.opt`;
  await run(
    process.execPath,
    [wasmOptCommand(), wasmPath, ...wasmOptArgs, '-o', optimizedPath],
    dirname(wasmPath),
  );
  await rename(optimizedPath, wasmPath);
}

function isInside(parent, child) {
  const path = relative(parent, child);
  return path === '' || (!path.startsWith(`..${sep}`) && path !== '..');
}

function errorOverlaySource() {
  return `
function reportError(error) {
  console.error('Waluau game failed:', error);
  const message = typeof error === 'string' ? error : error?.message || String(error);
  const box = document.createElement('pre');
  box.id = 'waluau-error';
  box.setAttribute(
    'style',
    'position:fixed;inset:0;z-index:2147483647;margin:0;padding:1rem;overflow:auto;white-space:pre-wrap;font:14px/1.5 ui-monospace,monospace;color:#fecaca;background:#18090b',
  );
  box.textContent = message;
  document.body.appendChild(box);
}
`;
}

function shaderModuleSource(shaderSources) {
  const shaderImports = shaderSources.map(
    ({ specifier }, index) => `import waluauShaderSource${index} from ${JSON.stringify(specifier)};`,
  ).join('\n');
  const initialShaderSources = shaderSources.map(
    ({ name }, index) => `[${JSON.stringify(name)}]: waluauShaderSource${index}`,
  ).join(',\n  ');
  // Accept each dependency independently. Vite may update only one member of
  // a multi-source shader set, so a positional multi-dependency callback
  // would risk associating an unchanged/undefined module with the wrong key.
  const shaderHotAccepts = shaderSources.map(
    ({ name, specifier }) => `
  import.meta.hot.accept(${JSON.stringify(specifier)}, (module) => {
    shaderSourceHost.update(${JSON.stringify(name)}, module?.default);
  });`,
  ).join('\n');
  return `
import { createWaluauShaderSourceHost } from '@waluau/vite-plugin/shaders';
${shaderImports}

const shaderSourceHost = createWaluauShaderSourceHost({
  ${initialShaderSources}
});

if (import.meta.hot) {
${shaderHotAccepts}
}

export default shaderSourceHost;
`;
}

function runtimeSource(generatedModule, version, shaderModule) {
  const generatedSpecifier = `${generatedModule}?waluau-hmr=${version}`;
  return `
import { buildWaluauImports } from '@waluau/vite-plugin/runtime';
import {
  captureWaluauGame,
  createWaluauHotHost,
  HotReplacementFallback,
  replaceWaluauGame,
} from '@waluau/vite-plugin/hot';
import shaderSourceHost from ${JSON.stringify(shaderModule)};
import {
  run as runWaluau,
  wasmUrl as generatedWasmUrl,
} from ${JSON.stringify(generatedSpecifier)};

${errorOverlaySource()}

function versionedWasmUrl() {
  if (generatedWasmUrl == null) return null;
  const url = new URL(generatedWasmUrl);
  url.searchParams.set('waluau-hmr', ${JSON.stringify(String(version))});
  return url;
}

const hotReplacement = createWaluauHotHost();

function startGame() {
  return runWaluau({
    wasmUrl: versionedWasmUrl(),
    createImports: (context) => buildWaluauImports(null, console.log, {
      requiredImports: context.requiredImports,
      bytesConstants: context.bytesConstants,
      domOutputRoot: document,
      getWasmExports: context.getWasmExports,
      onAsyncError: reportError,
      hotReplacement,
      shaderSources: shaderSourceHost,
      gameServices: {
        assetBaseUrl: context.assetBaseUrl,
        assetManifest: context.assetManifest,
      },
    }),
  }).then((loaded) => ({ ...loaded, hotReplacement }));
}

export const game = replaceWaluauGame({
  previous: import.meta.hot?.data.waluauGame,
  start: startGame,
  reload: (reason) => import.meta.hot?.invalidate(reason),
});

if (import.meta.hot) {
  import.meta.hot.accept();
  import.meta.hot.dispose((data) => {
    data.waluauGame = captureWaluauGame(game);
  });
}

void game.catch((error) => {
  if (!(error instanceof HotReplacementFallback)) reportError(error);
});
export default game;
`;
}

// A *.stories.walu file becomes a Component Story Format module: one named
// export per published story, all of them mounting through the same book, so
// Storybook's own indexer contract is met by ordinary JavaScript exports. The
// Wasm module is not touched until a story is actually rendered.
function storiesModuleSource(generatedModule, version, shaderModule, stories) {
  const generatedSpecifier = `${generatedModule}?waluau-hmr=${version}`;
  const exports = stories.map(
    ({ name, exportName, args, argTypes }) => `export const ${exportName} = {
  name: ${JSON.stringify(name)},
${args == null ? '' : `  args: ${JSON.stringify(args)},
  argTypes: ${JSON.stringify(argTypes)},
`}  render: (args) => ({ book, name: ${JSON.stringify(name)}, args }),
};`,
  ).join('\n');
  return `
import { buildWaluauImports } from '@waluau/vite-plugin/runtime';
import { createWaluauBook } from '@waluau/vite-plugin/storybook';
import shaderSourceHost from ${JSON.stringify(shaderModule)};
import {
  run as runWaluau,
  wasmUrl as generatedWasmUrl,
} from ${JSON.stringify(generatedSpecifier)};

// The generated module is imported per build; its sibling Wasm URL is not, so
// it carries the same version to keep a recompiled story off the old binary.
function versionedWasmUrl() {
  if (generatedWasmUrl == null) return null;
  const url = new URL(generatedWasmUrl);
  url.searchParams.set('waluau-hmr', ${JSON.stringify(String(version))});
  return url;
}

const book = createWaluauBook({
  run: runWaluau,
  wasmUrl: versionedWasmUrl(),
  createImports: (context, hostImports) => buildWaluauImports(null, console.log, {
    requiredImports: context.requiredImports,
    bytesConstants: context.bytesConstants,
    domOutputRoot: document,
    getWasmExports: context.getWasmExports,
    hostImports,
    shaderSources: shaderSourceHost,
    gameServices: {
      assetBaseUrl: context.assetBaseUrl,
      assetManifest: context.assetManifest,
    },
  }),
});

export default {
  // A story is a fixed-size canvas, so it is centred in the preview rather
  // than laid out in the flow of a page.
  parameters: { layout: 'centered' },
};

${exports}
`;
}

function testModuleSource(generatedModule) {
  return `
import { run } from ${JSON.stringify(generatedModule)};
import { registerWaluGlueTests } from '@waluau/vite-plugin/testing';

await registerWaluGlueTests({ run });
`;
}

function fullScreenStyle() {
  return `
html, body {
  width: 100%;
  height: 100%;
  margin: 0;
  overflow: hidden;
}

#walua-game {
  width: 100vw !important;
  height: 100vh !important;
  min-height: 100vh !important;
  padding: 0 !important;
}
#walua-game-canvas {
  display: block !important;
  width: 100vw !important;
  height: 100vh !important;
  max-width: none !important;
  max-height: none !important;
  aspect-ratio: auto !important;
}
`;
}

function importManifestAssets(code) {
  const imports = [];
  let assetIndex = 0;
  const transformed = code.replace(
    /(export const assetManifest = Object\.freeze\(\{[\s\S]*?\}\);)/,
    (manifest) => manifest.replace(/url: ("(?:[^"\\]|\\.)*")/g, (_, encodedUrl) => {
      const binding = `waluauAssetUrl${assetIndex++}`;
      imports.push(`import ${binding} from ${JSON.stringify(`${JSON.parse(encodedUrl)}?url`)};`);
      return `url: ${binding}`;
    }),
  );
  return imports.length === 0 ? transformed : `${imports.join('\n')}\n${transformed}`;
}

/**
 * Compile imported `.walu` files into browser-hosted Waluau game modules.
 *
 * @param {{
 *   fullScreen?: boolean,
 *   manifest?: string,
 *   optimize?: boolean,
 *   shaderSources?: Record<string, string>,
 *   workspaceRoot?: string,
 *   compiler?: { command: string, args?: string[], persistent?: boolean }
 * }} options
 *   `optimize` (default true) runs binaryen's wasm-opt over the compiled Wasm
 *   in production builds, roughly halving the module size. Dev-server and
 *   vitest builds never optimize; hot-reload latency wins there.
 */
export function waluau(options = {}) {
  let appRoot = process.cwd();
  let cacheRoot = resolve(appRoot, '.waluau');
  let server;
  let compilerHost;
  let productionBuild = false;

  const workspaceRoot = resolve(options.workspaceRoot ?? repositoryRoot);
  const fullScreen = options.fullScreen ?? true;
  const optimize = options.optimize ?? true;
  if (
    options.shaderSources != null
    && (Array.isArray(options.shaderSources) || typeof options.shaderSources !== 'object')
  ) {
    throw new TypeError('shaderSources must be a name-to-path object');
  }
  const configuredShaderSources = Object.entries(options.shaderSources ?? {}).map(
    ([name, path]) => {
      if (name.length === 0) {
        throw new TypeError('shaderSources names must be non-empty strings');
      }
      if (typeof path !== 'string' || path.length === 0) {
        throw new TypeError(`shaderSources["${name}"] must be a non-empty path`);
      }
      return { name, path };
    },
  );
  const compileStates = new Map();
  const compiledEntries = new Set();
  const generatedModules = new Set();
  const shaderModules = new Map();

  function resolvePaths(root) {
    appRoot = root;
    cacheRoot = resolve(appRoot, '.waluau');
  }

  function artifactPaths(entryPath) {
    const key = createHash('sha256').update(entryPath).digest('hex').slice(0, 12);
    const outDir = resolve(cacheRoot, key);
    return {
      outDir,
      wasm: resolve(outDir, 'game.wasm'),
      module: resolve(outDir, 'game.js'),
      report: resolve(outDir, 'report.json'),
    };
  }

  function resolvedShaderSources() {
    return configuredShaderSources.map(({ name, path }) => {
      const file = resolve(appRoot, path);
      return { name, file, specifier: `${file}?raw` };
    });
  }

  function isConfiguredShaderSource(file) {
    return resolvedShaderSources().some((source) => source.file === file);
  }

  function compilerBuildArgs(entryPath, wasmOutput, reportOutput) {
    const manifestArgs = options.manifest == null
      ? []
      : ['--manifest', resolve(appRoot, options.manifest)];
    const reportArgs = ['--report', reportOutput];
    return [entryPath, '-o', wasmOutput, '--emit-js', ...manifestArgs, ...reportArgs];
  }

  function compilerCommand(buildArgs) {
    if (options.compiler) {
      return {
        command: options.compiler.command,
        args: [...(options.compiler.args ?? []), ...buildArgs],
        cwd: appRoot,
      };
    }
    if (existsSync(resolve(workspaceRoot, 'Cargo.toml'))) {
      return {
        command: 'cargo',
        args: [
          'run',
          '--quiet',
          '--release',
          '-p',
          'waluau-cli',
          '--',
          ...buildArgs,
        ],
        cwd: workspaceRoot,
      };
    }
    return {
      command: 'waluau',
      args: buildArgs,
      cwd: appRoot,
    };
  }

  function compilerServerCommand() {
    if (options.compiler) {
      if (options.compiler.persistent !== true) return null;
      return {
        command: options.compiler.command,
        args: [...(options.compiler.args ?? []), '--server'],
        cwd: appRoot,
      };
    }
    if (existsSync(resolve(workspaceRoot, 'Cargo.toml'))) {
      return {
        command: 'cargo',
        args: ['run', '--quiet', '--release', '-p', 'waluau-cli', '--', '--server'],
        cwd: workspaceRoot,
      };
    }
    return { command: 'waluau', args: ['--server'], cwd: appRoot };
  }

  async function executeCompiler(buildArgs) {
    const serverCommand = compilerServerCommand();
    if (serverCommand == null) {
      const invocation = compilerCommand(buildArgs);
      return run(invocation.command, invocation.args, invocation.cwd);
    }
    compilerHost ??= createCompilerHost(serverCommand);
    return compilerHost.build(buildArgs);
  }

  async function restartCompilerHost() {
    if (compilerHost == null) return;
    await compilerHost.restart();
  }

  async function closeCompilerHost() {
    if (compilerHost == null) return;
    const activeHost = compilerHost;
    compilerHost = undefined;
    await activeHost.close();
  }

  /** Read the compiler's build report; null when missing or unparsable. */
  function readBuildReport(reportPath) {
    try {
      return JSON.parse(readFileSync(reportPath, 'utf8'));
    } catch {
      return null;
    }
  }

  function manifestFiles() {
    if (options.manifest == null) return [];
    const manifestPath = resolve(appRoot, options.manifest);
    try {
      const { assets = [] } = JSON.parse(readFileSync(manifestPath, 'utf8'));
      return assets
        .filter(({ path }) => typeof path === 'string')
        .map(({ path }) => resolve(dirname(manifestPath), path));
    } catch {
      return [];
    }
  }

  async function compileEntry(entryPath, { force = false } = {}) {
    const artifacts = artifactPaths(entryPath);
    let state = compileStates.get(entryPath);
    if (!state) {
      state = {
        fresh: false,
        inFlight: null,
        queued: false,
        involvedFiles: null,
        version: 0,
      };
      compileStates.set(entryPath, state);
    }
    if (state.inFlight) {
      if (force) state.queued = true;
      await state.inFlight;
      return artifacts;
    }
    // `handleHotUpdate` prepares the new artifacts before invalidating the
    // entry module. Vite then transforms that module again; reuse the prepared
    // generation instead of compiling the same source a second time.
    if (!force && state.fresh) return artifacts;

    state.inFlight = (async () => {
      do {
        state.queued = false;
        state.fresh = false;
        await mkdir(artifacts.outDir, { recursive: true });
        const buildArgs = compilerBuildArgs(entryPath, artifacts.wasm, artifacts.report);
        try {
          await executeCompiler(buildArgs);
          // Only production builds pay for wasm-opt (a few seconds per
          // module); the dev-server/HMR path always serves the compiler's
          // direct output.
          if (productionBuild && optimize) await optimizeWasm(artifacts.wasm);
          state.version += 1;
          state.fresh = true;
        } finally {
          // The report is written even for failed builds, so watch mode can
          // still track every file in the entry's require graph.
          const report = readBuildReport(artifacts.report);
          if (Array.isArray(report?.involvedFiles)) {
            state.involvedFiles = new Set(report.involvedFiles.map((file) => resolve(file)));
          }
        }
      } while (state.queued);
    })().finally(() => {
      state.inFlight = null;
    });

    await state.inFlight;
    compiledEntries.add(entryPath);
    generatedModules.add(artifacts.module);
    return artifacts;
  }

  /** Compiler-internal inputs (not in build reports) that affect every entry. */
  function affectsCompilerProcess(file) {
    if (options.compiler != null) return false;
    return (
      file === resolve(workspaceRoot, 'Cargo.toml') ||
      file === resolve(workspaceRoot, 'Cargo.lock') ||
      isInside(resolve(workspaceRoot, 'crates'), file) ||
      isInside(resolve(workspaceRoot, 'engine'), file) ||
      isInside(resolve(workspaceRoot, 'builtins'), file) ||
      isInside(resolve(workspaceRoot, 'externs'), file)
    );
  }

  function affectsAllEntries(file) {
    if (options.manifest != null && file === resolve(appRoot, options.manifest)) return true;
    if (manifestFiles().includes(file)) return true;
    return affectsCompilerProcess(file);
  }

  /** Entries whose last build involved `file`; all entries when unknown. */
  function entriesInvolving(file) {
    if (affectsAllEntries(file)) return Array.from(compiledEntries);
    return Array.from(compiledEntries).filter((entry) => {
      const involved = compileStates.get(entry)?.involvedFiles;
      return involved == null || involved.has(file) || entry === file;
    });
  }

  function watchesGameSource(file) {
    if (options.manifest != null && file === resolve(appRoot, options.manifest)) return true;
    if (manifestFiles().includes(file)) return true;
    if (affectsCompilerProcess(file)) return true;
    return file.endsWith('.walu') && (
      isInside(appRoot, file) ||
      isInside(resolve(workspaceRoot, 'engine'), file) ||
      isInside(resolve(workspaceRoot, 'builtins'), file) ||
      isInside(resolve(workspaceRoot, 'externs'), file)
    );
  }

  function isGeneratedArtifact(file) {
    return Array.from(compiledEntries).some(
      (entry) => isInside(artifactPaths(entry).outDir, file),
    );
  }

  resolvePaths(appRoot);

  return {
    name: 'waluau-game',
    enforce: 'pre',
    configResolved(config) {
      resolvePaths(config.root);
      // `vite build` and `build-storybook` resolve with command 'build'; the
      // dev server and vitest resolve with 'serve'.
      productionBuild = config.command === 'build';
    },
    async transform(code, id) {
      const file = id.split('?')[0];
      if (generatedModules.has(file)) {
        return importManifestAssets(code).replace(
          "new URL('./', import.meta.url)",
          "new URL(/* @vite-ignore */ './', import.meta.url)",
        );
      }
      if (id.includes('?') || !file.endsWith('.walu')) return null;

      this.addWatchFile(file);
      if (options.manifest != null) {
        this.addWatchFile(resolve(appRoot, options.manifest));
        for (const asset of manifestFiles()) this.addWatchFile(asset);
      }
      const artifacts = await compileEntry(file);
      // Watch every module the build reported as involved, so edits to
      // transitively-required files rebuild even outside the default roots.
      for (const involved of compileStates.get(file)?.involvedFiles ?? []) {
        this.addWatchFile(involved);
      }
      // *.test.walu files register with vitest and *.stories.walu files become
      // Storybook CSF modules, instead of booting a game.
      const isTestModule = file.endsWith('.test.walu');
      const isStoriesModule = file.endsWith('.stories.walu');
      const shaderModule = `${shaderModulePrefix}${createHash('sha256')
        .update(file)
        .digest('hex')
        .slice(0, 12)}`;
      shaderModules.set(`\0${shaderModule}`, shaderModuleSource(resolvedShaderSources()));
      if (isTestModule) return { code: testModuleSource(artifacts.module), map: null };
      if (isStoriesModule) {
        return {
          code: storiesModuleSource(
            artifacts.module,
            compileStates.get(file).version,
            shaderModule,
            parseStories(code),
          ),
          map: null,
        };
      }
      return {
        code: runtimeSource(
          artifacts.module,
          compileStates.get(file).version,
          shaderModule,
        ),
        map: null,
      };
    },
    resolveId(id) {
      if (id.startsWith(shaderModulePrefix)) return `\0${id}`;
      return null;
    },
    load(id) {
      return shaderModules.get(id) ?? null;
    },
    transformIndexHtml() {
      if (!fullScreen) return [];
      return [{ tag: 'style', children: fullScreenStyle(), injectTo: 'head' }];
    },
    configureServer(viteServer) {
      server = viteServer;
      if (existsSync(resolve(workspaceRoot, 'engine'))) {
        viteServer.watcher.add(resolve(workspaceRoot, 'engine'));
        viteServer.watcher.add(resolve(workspaceRoot, 'builtins'));
        viteServer.watcher.add(resolve(workspaceRoot, 'externs'));
        viteServer.watcher.add(resolve(workspaceRoot, 'crates'));
        viteServer.watcher.add(resolve(workspaceRoot, 'Cargo.toml'));
        viteServer.watcher.add(resolve(workspaceRoot, 'Cargo.lock'));
      }
      viteServer.httpServer?.once('close', () => void closeCompilerHost());
    },
    async handleHotUpdate(context) {
      const file = resolve(context.file);
      // The compiler writes JS, Wasm, reports, and copied assets under the
      // entry's cache directory. Suppress their separate watcher events;
      // otherwise Vite can apply stale dependency updates or full reloads
      // before the versioned source-entry update, then recompile in a loop.
      if (isGeneratedArtifact(file)) return [];
      // Configured shader files belong to Vite's raw dependency graph, even
      // when a path also ends in .walu or appeared in a compiler build report.
      // Let the generated dependency-specific accept callback handle it before
      // any Waluau rebuild/invalidation logic can observe the edit.
      if (isConfiguredShaderSource(file)) return;
      if (compiledEntries.size === 0) return;
      const isKnownInvolved = Array.from(compiledEntries).some(
        (entry) => compileStates.get(entry)?.involvedFiles?.has(file),
      );
      if (!isKnownInvolved && !watchesGameSource(file)) return;
      // Rebuild only the entries whose require graph contains the changed
      // file (all of them when a build never produced a report).
      const entries = entriesInvolving(file);
      if (entries.length === 0) return;
      // Embedded engine/compiler inputs require a fresh process so Cargo can
      // rebuild the binary. Ordinary game edits retain the live session.
      if (affectsCompilerProcess(file)) await restartCompilerHost();
      await Promise.all(entries.map((entry) => compileEntry(entry, { force: true })));
      const reloadServer = server ?? context.server;
      const modules = [];
      for (const entry of entries) {
        const entryModules = reloadServer.moduleGraph?.getModulesByFile(entry);
        if (entryModules) {
          for (const module of entryModules) {
            reloadServer.moduleGraph.invalidateModule(module);
            modules.push(module);
          }
        } else if (entry === file) {
          modules.push(...(context.modules ?? []));
        }
      }
      if (modules.length === 0) {
        // No browser module can accept this rebuild (for example, the entry
        // was removed from Vite's graph). A full reload is the only safe way
        // to make the newly compiled Wasm reachable.
        reloadServer.ws.send({ type: 'full-reload' });
        return [];
      }
      return modules;
    },
    async closeBundle() {
      await closeCompilerHost();
    },
  };
}
