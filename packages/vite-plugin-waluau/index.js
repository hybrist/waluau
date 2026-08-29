import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, readFileSync } from 'node:fs';
import { copyFile, mkdir, rename, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { dirname, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

import { createCompilerHost } from './compiler-host.js';
import { parseStories } from './stories.js';

const packageRoot = dirname(fileURLToPath(import.meta.url));
const repositoryRoot = resolve(packageRoot, '../..');
const shaderModulePrefix = 'virtual:waluau-shader-sources:';
const developmentSourcePath = /^__waluau\/sources\/(?:files|packages|virtual)\/s-(?:[A-Za-z0-9._-]|~[0-9A-F]{2})*(?:\/s-(?:[A-Za-z0-9._-]|~[0-9A-F]{2})*)*$/;

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

function testModuleSource(generatedModule, version) {
  const generatedSpecifier = `${generatedModule}?waluau-hmr=${version}`;
  return `
import {
  run,
  wasmUrl as generatedWasmUrl,
} from ${JSON.stringify(generatedSpecifier)};
import { registerWaluGlueTests } from '@waluau/vite-plugin/testing';

function versionedWasmUrl() {
  if (generatedWasmUrl == null) return null;
  const url = new URL(generatedWasmUrl);
  url.searchParams.set('waluau-hmr', ${JSON.stringify(String(version))});
  return url;
}

await registerWaluGlueTests({ run, wasmUrl: versionedWasmUrl() });
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

  function commonSourceDirectory(files) {
    const sourceFiles = Array.from(files);
    if (sourceFiles.length === 0) return null;
    let common = dirname(sourceFiles[0]);
    for (const file of sourceFiles.slice(1)) {
      const directory = dirname(file);
      while (!isInside(common, directory)) {
        const parent = dirname(common);
        if (parent === common) return null;
        common = parent;
      }
    }
    return common;
  }

  async function materializeDwarfSources(outDir, developmentSources, involvedFiles) {
    if (Array.isArray(developmentSources)) {
      const sourceRoot = resolve(outDir, '__waluau', 'sources');
      const snapshots = developmentSources.map(({ path, source }) => {
        if (typeof path !== 'string' || typeof source !== 'string') {
          throw new Error('developmentSources entries require string path and source fields');
        }
        // Only the compiler's canonical, unreserved-only encoding is accepted.
        // Vite leaves these paths byte-for-byte unchanged at its filesystem
        // boundary, so distinct authored filenames cannot alias one another.
        if (!developmentSourcePath.test(path)) {
          throw new Error(`invalid development source path: ${path}`);
        }
        const destination = resolve(outDir, path);
        if (destination === sourceRoot || !isInside(sourceRoot, destination)) {
          throw new Error(`development source path escapes its reserved directory: ${path}`);
        }
        return { destination, path, source };
      });
      const destinations = new Set();
      for (const { destination, path } of snapshots) {
        if (destinations.has(destination)) {
          throw new Error(`duplicate development source destination: ${path}`);
        }
        destinations.add(destination);
      }
      await Promise.all(snapshots.map(async ({ destination, source }) => {
        await mkdir(dirname(destination), { recursive: true });
        await writeFile(destination, source);
      }));
      return;
    }
    // A file can disappear between a successful compile and this snapshot
    // from an older/custom compiler's build report. This fallback preserves
    // filesystem source debugging, but only the developmentSources contract
    // can provide embedded package inputs and in-memory overlays exactly.
    const sourceFiles = Array.from(involvedFiles).filter((file) => existsSync(file));
    const sourceRoot = commonSourceDirectory(sourceFiles);
    if (sourceRoot == null) return;
    await Promise.all(sourceFiles.map(async (file) => {
      const destination = resolve(outDir, relative(sourceRoot, file));
      if (!isInside(outDir, destination)) return;
      await mkdir(dirname(destination), { recursive: true });
      await copyFile(file, destination);
    }));
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

  // Production game modules only need the runtime entry points; the broad
  // per-function export surface exists for the playground, and for test and
  // story modules whose functions are reached from the host. Tests and
  // stories keep full exports even in `vite build` (build-storybook).
  function usesMinimalExports(entryPath) {
    return (
      productionBuild
      && !entryPath.endsWith('.test.walu')
      && !entryPath.endsWith('.stories.walu')
    );
  }

  function compilerBuildArgs(entryPath, wasmOutput, reportOutput) {
    const dwarfArgs = productionBuild ? [] : ['--development-dwarf'];
    const manifestArgs = options.manifest == null
      ? []
      : ['--manifest', resolve(appRoot, options.manifest)];
    const reportArgs = ['--report', reportOutput];
    const exportArgs = usesMinimalExports(entryPath) ? ['--minimal-exports'] : [];
    return [
      entryPath,
      '-o', wasmOutput,
      '--emit-js',
      ...dwarfArgs,
      ...exportArgs,
      ...manifestArgs,
      ...reportArgs,
    ];
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

  /** Convert compiler metadata into the error shape consumed by Vite/Rollup. */
  function compilerError(error, report, entryPath) {
    const diagnostics = Array.isArray(error?.diagnostics) && error.diagnostics.length > 0
      ? error.diagnostics
      : (Array.isArray(report?.diagnostics) ? report.diagnostics : []);
    const diagnostic = diagnostics.find(({ severity }) => severity === 'error') ?? diagnostics[0];
    if (diagnostic == null) return error;

    const failure = new Error(diagnostic.message ?? error?.message ?? 'Waluau compilation failed', {
      cause: error,
    });
    failure.plugin = 'waluau-game';
    failure.id = typeof diagnostic.file === 'string' ? diagnostic.file : entryPath;
    if (Number.isInteger(diagnostic.line) && Number.isInteger(diagnostic.column)) {
      failure.loc = {
        file: failure.id,
        line: diagnostic.line,
        column: Math.max(0, diagnostic.column - 1),
      };
    }
    failure.diagnostics = diagnostics;
    return failure;
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
        developmentSources: null,
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
    if (!force && state.fresh) {
      state.fresh = false;
      return artifacts;
    }

    state.inFlight = (async () => {
      do {
        state.queued = false;
        state.fresh = false;
        await mkdir(artifacts.outDir, { recursive: true });
        const buildArgs = compilerBuildArgs(entryPath, artifacts.wasm, artifacts.report);
        let compileFailure;
        let report;
        try {
          await executeCompiler(buildArgs);
          // Only production builds pay for wasm-opt (a few seconds per
          // module); the dev-server/HMR path always serves the compiler's
          // direct output.
          if (productionBuild && optimize) await optimizeWasm(artifacts.wasm);
        } catch (error) {
          compileFailure = error;
        } finally {
          // The report is written even for failed builds, so watch mode can
          // still track every file in the entry's require graph.
          report = readBuildReport(artifacts.report);
          if (Array.isArray(report?.involvedFiles)) {
            state.involvedFiles = new Set(report.involvedFiles.map((file) => resolve(file)));
          }
          state.developmentSources = Array.isArray(report?.developmentSources)
            ? report.developmentSources
            : null;
        }
        if (compileFailure != null) {
          throw compilerError(compileFailure, report, entryPath);
        }
        // DWARF paths are relative to the common authored source directory and
        // Chrome resolves them beside the Wasm URL. Snapshot the successful
        // dev build's source graph there so Vite can serve those exact URLs.
        if (
          !productionBuild
          && (state.developmentSources != null || state.involvedFiles != null)
        ) {
          await materializeDwarfSources(
            artifacts.outDir,
            state.developmentSources,
            state.involvedFiles,
          );
        }
        state.version += 1;
        state.fresh = force;
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
    config(config) {
      const root = resolve(config.root ?? process.cwd());
      const cacheRoot = resolve(root, '.waluau').split(sep).join('/');
      return { server: { watch: { ignored: [`${cacheRoot}/**`] } } };
    },
    configResolved(config) {
      resolvePaths(config.root);
      // `vite build` and `build-storybook` resolve with command 'build'; the
      // dev server and vitest resolve with 'serve'.
      productionBuild = config.command === 'build';
    },
    async transform(code, id) {
      const [file, query] = id.split('?');
      if (generatedModules.has(file)) {
        return importManifestAssets(code).replace(
          "new URL('./', import.meta.url)",
          "new URL(/* @vite-ignore */ './', import.meta.url)",
        );
      }
      const searchParams = new URLSearchParams(query);
      if (!file.endsWith('.walu') || searchParams.has('raw') || searchParams.has('url')) return null;

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
      if (isTestModule) {
        return {
          code: testModuleSource(
            artifacts.module,
            compileStates.get(file).version,
          ),
          map: null,
        };
      }
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
