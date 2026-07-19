import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync } from 'node:fs';
import { mkdir } from 'node:fs/promises';
import { dirname, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

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

function runtimeSource(generatedModule) {
  return `
import { buildWaluauImports } from '@waluau/vite-plugin/runtime';
import { run as runWaluau } from ${JSON.stringify(generatedModule)};

${errorOverlaySource()}

export const game = runWaluau({
  createImports: (context) => buildWaluauImports(null, console.log, {
    requiredImports: context.requiredImports,
    bytesConstants: context.bytesConstants,
    domOutputRoot: document,
    getWasmExports: context.getWasmExports,
    onAsyncError: reportError,
    gameServices: {
      assetBaseUrl: context.assetBaseUrl,
      assetManifest: context.assetManifest,
    },
  }),
});

void game.catch(reportError);
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

/**
 * Compile imported `.walu` files into browser-hosted Waluau game modules.
 *
 * @param {{
 *   fullScreen?: boolean,
 *   workspaceRoot?: string,
 *   compiler?: { command: string, args?: string[] }
 * }} options
 */
export function waluau(options = {}) {
  let appRoot = process.cwd();
  let cacheRoot = resolve(appRoot, '.waluau');
  let server;

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
    };
  }

  function compilerCommand(entryPath, wasmOutput) {
    if (options.compiler) {
      return {
        command: options.compiler.command,
        args: [...(options.compiler.args ?? []), entryPath, '-o', wasmOutput, '--emit-js'],
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
          entryPath,
          '-o',
          wasmOutput,
          '--emit-js',
        ],
        cwd: workspaceRoot,
      };
    }
    return {
      command: 'waluau',
      args: [entryPath, '-o', wasmOutput, '--emit-js'],
      cwd: appRoot,
    };
  }

  async function compileEntry(entryPath) {
    const artifacts = artifactPaths(entryPath);
    let state = compileStates.get(entryPath);
    if (!state) {
      state = { inFlight: null, queued: false };
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
        const invocation = compilerCommand(entryPath, artifacts.wasm);
        await run(invocation.command, invocation.args, invocation.cwd);
      } while (state.queued);
    })().finally(() => {
      state.inFlight = null;
    });

    await state.inFlight;
    compiledEntries.add(entryPath);
    generatedModules.add(artifacts.module);
    return artifacts;
  }

  function watchesGameSource(file) {
    return file.endsWith('.walu') && (
      isInside(appRoot, file) ||
      isInside(resolve(workspaceRoot, 'engine'), file) ||
      isInside(resolve(workspaceRoot, 'builtins'), file) ||
      isInside(resolve(workspaceRoot, 'externs'), file)
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
        return code.replace(
          "new URL('./', import.meta.url)",
          "new URL(/* @vite-ignore */ './', import.meta.url)",
        );
      }
      if (id.includes('?') || !file.endsWith('.walu')) return null;

      this.addWatchFile(file);
      const artifacts = await compileEntry(file);
      // *.test.walu files register with vitest instead of booting a game.
      const isTestModule = file.endsWith('.test.walu');
      return {
        code: isTestModule ? testModuleSource(artifacts.module) : runtimeSource(artifacts.module),
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
      }
    },
    async handleHotUpdate(context) {
      const file = resolve(context.file);
      if (!watchesGameSource(file) || compiledEntries.size === 0) return;
      await Promise.all(Array.from(compiledEntries, (entry) => compileEntry(entry)));
      (server ?? context.server).ws.send({ type: 'full-reload' });
      return [];
    },
  };
}
