import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, readFileSync } from 'node:fs';
import { mkdir } from 'node:fs/promises';
import { dirname, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

import { createCompilerHost } from './compiler-host.js';

const packageRoot = dirname(fileURLToPath(import.meta.url));
const repositoryRoot = resolve(packageRoot, '../..');

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

function runtimeSource(generatedModule, version) {
  const generatedSpecifier = `${generatedModule}?waluau-hmr=${version}`;
  return `
import { buildWaluauImports } from '@waluau/vite-plugin/runtime';
import {
  captureWaluauGame,
  createWaluauHotHost,
  HotReplacementFallback,
  replaceWaluauGame,
} from '@waluau/vite-plugin/hot';
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
 *   workspaceRoot?: string,
 *   compiler?: { command: string, args?: string[], persistent?: boolean }
 * }} options
 */
export function waluau(options = {}) {
  let appRoot = process.cwd();
  let cacheRoot = resolve(appRoot, '.waluau');
  let server;
  let compilerHost;

  const workspaceRoot = resolve(options.workspaceRoot ?? repositoryRoot);
  const fullScreen = options.fullScreen ?? true;
  const compileStates = new Map();
  const compiledEntries = new Set();
  const generatedModules = new Set();

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
        args: ['run', '--quiet', '-p', 'waluau-cli', '--', '--server'],
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

  async function compileEntry(entryPath) {
    const artifacts = artifactPaths(entryPath);
    let state = compileStates.get(entryPath);
    if (!state) {
      state = { inFlight: null, queued: false, involvedFiles: null, version: 0 };
      compileStates.set(entryPath, state);
    }
    if (state.inFlight) {
      state.queued = true;
      await state.inFlight;
      return artifacts;
    }

    state.inFlight = (async () => {
      do {
        state.queued = false;
        await mkdir(artifacts.outDir, { recursive: true });
        const buildArgs = compilerBuildArgs(entryPath, artifacts.wasm, artifacts.report);
        try {
          await executeCompiler(buildArgs);
          state.version += 1;
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
      // *.test.walu files register with vitest instead of booting a game.
      const isTestModule = file.endsWith('.test.walu');
      return {
        code: isTestModule
          ? testModuleSource(artifacts.module)
          : runtimeSource(artifacts.module, compileStates.get(file).version),
        map: null,
      };
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
      await Promise.all(entries.map((entry) => compileEntry(entry)));
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
