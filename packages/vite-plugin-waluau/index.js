import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdir, readFile } from 'node:fs/promises';
import { dirname, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const VIRTUAL_GAME_ID = 'virtual:waluau-game';
const RESOLVED_VIRTUAL_GAME_ID = `\0${VIRTUAL_GAME_ID}`;
const DEFAULT_ENTRY = 'src/main.walu';

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
import { run } from ${JSON.stringify(generatedModule)};

${errorOverlaySource()}

void run({
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
}).catch(reportError);
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
 * Compile and host a Waluau game as the entry point of a Vite app.
 *
 * @param {{
 *   entry?: string,
 *   fullScreen?: boolean,
 *   workspaceRoot?: string,
 *   compiler?: { command: string, args?: string[] }
 * }} options
 */
export function waluau(options = {}) {
  let appRoot = process.cwd();
  let entryPath;
  let cacheDir;
  let generatedWasm;
  let generatedModule;
  let server;
  let compiling = null;
  let compileAgain = false;
  let reloadAfterCompile = false;

  const workspaceRoot = resolve(options.workspaceRoot ?? repositoryRoot);
  const fullScreen = options.fullScreen ?? true;

  function resolvePaths(root) {
    appRoot = root;
    entryPath = resolve(appRoot, options.entry ?? DEFAULT_ENTRY);
    cacheDir = resolve(appRoot, '.waluau');
    generatedWasm = resolve(cacheDir, 'game.wasm');
    generatedModule = resolve(cacheDir, 'game.js');
  }

  function compilerCommand() {
    if (options.compiler) {
      return {
        command: options.compiler.command,
        args: [...(options.compiler.args ?? []), entryPath, '-o', generatedWasm, '--emit-js'],
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
          generatedWasm,
          '--emit-js',
        ],
        cwd: workspaceRoot,
      };
    }
    return {
      command: 'waluau',
      args: [entryPath, '-o', generatedWasm, '--emit-js'],
      cwd: appRoot,
    };
  }

  async function compile() {
    if (compiling) {
      compileAgain = true;
      return compiling;
    }
    compiling = (async () => {
      await mkdir(cacheDir, { recursive: true });
      const invocation = compilerCommand();
      await run(invocation.command, invocation.args, invocation.cwd);
    })().finally(async () => {
      compiling = null;
      if (compileAgain) {
        compileAgain = false;
        await compile();
      }
      if (reloadAfterCompile && server) {
        reloadAfterCompile = false;
        server.ws.send({ type: 'full-reload' });
      }
    });
    return compiling;
  }

  function scheduleCompile() {
    reloadAfterCompile = true;
    void compile().catch((error) => {
      reloadAfterCompile = false;
      server?.config.logger.error(error.message);
      server?.ws.send({
        type: 'error',
        err: { message: error.message, stack: error.stack ?? error.message },
      });
    });
  }

  function watchesGameSource(file) {
    if (file === entryPath) return true;
    if (file.endsWith('.walu') && (
      isInside(appRoot, file) ||
      isInside(resolve(workspaceRoot, 'engine'), file) ||
      isInside(resolve(workspaceRoot, 'builtins'), file) ||
      isInside(resolve(workspaceRoot, 'externs'), file)
    )) {
      return true;
    }
    return false;
  }

  resolvePaths(appRoot);

  return {
    name: 'waluau-game',
    enforce: 'pre',
    configResolved(config) {
      resolvePaths(config.root);
    },
    async buildStart() {
      this.addWatchFile(entryPath);
      await compile();
    },
    resolveId(id) {
      if (id === VIRTUAL_GAME_ID) return RESOLVED_VIRTUAL_GAME_ID;
      return null;
    },
    async load(id) {
      if (id !== RESOLVED_VIRTUAL_GAME_ID) return null;
      await readFile(generatedModule, 'utf8');
      return runtimeSource(generatedModule);
    },
    transform(code, id) {
      if (id.split('?')[0] !== generatedModule) return null;
      return code.replace(
        "new URL('./', import.meta.url)",
        "new URL(/* @vite-ignore */ './', import.meta.url)",
      );
    },
    transformIndexHtml: {
      order: 'pre',
      handler() {
        return [
          ...(fullScreen
            ? [{ tag: 'style', children: fullScreenStyle(), injectTo: 'head' }]
            : []),
          {
            tag: 'script',
            attrs: { type: 'module' },
            children: `import ${JSON.stringify(VIRTUAL_GAME_ID)};`,
            injectTo: 'body',
          },
        ];
      },
    },
    configureServer(viteServer) {
      server = viteServer;
      viteServer.watcher.add(entryPath);
      if (existsSync(resolve(workspaceRoot, 'engine'))) {
        viteServer.watcher.add(resolve(workspaceRoot, 'engine'));
        viteServer.watcher.add(resolve(workspaceRoot, 'builtins'));
        viteServer.watcher.add(resolve(workspaceRoot, 'externs'));
      }
      viteServer.watcher.on('add', (file) => {
        if (watchesGameSource(resolve(file))) scheduleCompile();
      });
      viteServer.watcher.on('change', (file) => {
        if (watchesGameSource(resolve(file))) scheduleCompile();
      });
      viteServer.watcher.on('unlink', (file) => {
        if (watchesGameSource(resolve(file))) scheduleCompile();
      });
    },
  };
}
