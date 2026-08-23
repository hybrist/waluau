import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const fixtureDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(fixtureDir, '../..');
const usage = `usage: pnpm --filter ante exec node ../../fixtures/dwarf-chrome-wasm-gc/verify-compiler-chrome.mjs \\
  --chrome /path/to/chrome \\
  --extension /path/to/unpacked-cxx-devtools-extension [--headed]`;

function parseArguments(args) {
  const options = { headed: false };
  for (let index = 0; index < args.length; index += 1) {
    switch (args[index]) {
      case '--chrome':
        options.chrome = resolve(args[++index] ?? '');
        break;
      case '--extension':
        options.extension = resolve(args[++index] ?? '');
        break;
      case '--headed':
        options.headed = true;
        break;
      case '--help':
      case '-h':
        process.stdout.write(`${usage}\n`);
        process.exit(0);
        break;
      default:
        throw new Error(`unknown argument ${args[index]}\n${usage}`);
    }
  }
  if (!options.chrome || !options.extension) throw new Error(usage);
  if (!existsSync(options.chrome) || !statSync(options.chrome).isFile()) {
    throw new Error(`Chrome executable does not exist: ${options.chrome}`);
  }
  for (const file of ['manifest.json', 'DevToolsPluginHost.bundle.js']) {
    if (!existsSync(join(options.extension, file))) {
      throw new Error(`unpacked extension is missing ${file}: ${options.extension}`);
    }
  }
  return options;
}

function loadPlaywright() {
  const explicit = process.env.WALUAU_PLAYWRIGHT_PATH;
  if (explicit) return createRequire(import.meta.url)(explicit);
  try {
    return createRequire(join(repoRoot, 'apps/ante/package.json'))('playwright');
  } catch (error) {
    throw new Error(
      'Playwright is unavailable. Run pnpm install, or set WALUAU_PLAYWRIGHT_PATH ' +
      'to the installed playwright module directory.',
      { cause: error },
    );
  }
}

function run(command, args) {
  const result = spawnSync(command, args, { cwd: repoRoot, stdio: 'inherit' });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} exited with status ${result.status}`);
}

function readUleb(bytes, state) {
  let value = 0;
  let shift = 0;
  while (true) {
    const byte = bytes[state.offset++];
    value |= (byte & 0x7f) << shift;
    if ((byte & 0x80) === 0) return value >>> 0;
    shift += 7;
  }
}

function readName(bytes, state) {
  const length = readUleb(bytes, state);
  const value = new TextDecoder().decode(bytes.subarray(state.offset, state.offset + length));
  state.offset += length;
  return value;
}

function sections(bytes) {
  const result = [];
  const state = { offset: 8 };
  while (state.offset < bytes.length) {
    const start = state.offset;
    const id = bytes[state.offset++];
    const size = readUleb(bytes, state);
    const contentStart = state.offset;
    const end = contentStart + size;
    const customState = { offset: contentStart };
    const name = id === 0 ? readName(bytes, customState) : undefined;
    result.push({ id, name, start, contentStart, payloadStart: customState.offset, end });
    state.offset = end;
  }
  return result;
}

function skipLimits(bytes, state) {
  const flags = readUleb(bytes, state);
  readUleb(bytes, state);
  if (flags & 1) readUleb(bytes, state);
}

function skipRefType(bytes, state) {
  const type = bytes[state.offset++];
  if (type === 0x63 || type === 0x64) readUleb(bytes, state);
}

function importedFunctionCount(bytes, allSections) {
  const section = allSections.find(item => item.id === 2);
  if (!section) return 0;
  const state = { offset: section.contentStart };
  const count = readUleb(bytes, state);
  let functions = 0;
  for (let index = 0; index < count; index += 1) {
    readName(bytes, state);
    readName(bytes, state);
    switch (bytes[state.offset++]) {
      case 0:
        functions += 1;
        readUleb(bytes, state);
        break;
      case 1:
        skipRefType(bytes, state);
        skipLimits(bytes, state);
        break;
      case 2:
        skipLimits(bytes, state);
        break;
      case 3:
        skipRefType(bytes, state);
        state.offset += 1;
        break;
      case 4:
        state.offset += 1;
        readUleb(bytes, state);
        break;
      default:
        throw new Error('unsupported Wasm import kind');
    }
  }
  return functions;
}

function findExportedFunction(bytes, allSections, predicate) {
  const section = allSections.find(item => item.id === 7);
  const state = { offset: section.contentStart };
  const count = readUleb(bytes, state);
  for (let index = 0; index < count; index += 1) {
    const name = readName(bytes, state);
    const kind = bytes[state.offset++];
    const itemIndex = readUleb(bytes, state);
    if (kind === 0 && predicate(name)) return { name, index: itemIndex };
  }
  throw new Error('synthetic function export not found');
}

function functionInstructionOffset(bytes, allSections, functionIndex) {
  const code = allSections.find(item => item.id === 10);
  const target = functionIndex - importedFunctionCount(bytes, allSections);
  const state = { offset: code.contentStart };
  const count = readUleb(bytes, state);
  if (target < 0 || target >= count) throw new Error('function has no defined code body');
  for (let index = 0; index < count; index += 1) {
    const bodySize = readUleb(bytes, state);
    const bodyEnd = state.offset + bodySize;
    if (index === target) {
      const localGroupCount = readUleb(bytes, state);
      for (let group = 0; group < localGroupCount; group += 1) {
        readUleb(bytes, state);
        skipRefType(bytes, state);
      }
      return state.offset - code.contentStart;
    }
    state.offset = bodyEnd;
  }
  throw new Error('function body not found');
}

function inspectArtifacts(defaultPath, developmentPath, debugPath) {
  const normal = new Uint8Array(readFileSync(defaultPath));
  const development = new Uint8Array(readFileSync(developmentPath));
  const debug = new Uint8Array(readFileSync(debugPath));
  const normalSections = sections(normal);
  const developmentSections = sections(development);
  const debugSections = sections(debug);
  const normalCustom = normalSections.filter(item => item.id === 0).map(item => item.name);
  const developmentCustom = developmentSections.filter(item => item.id === 0).map(item => item.name);
  const debugCustom = debugSections.filter(item => item.id === 0).map(item => item.name);
  if (normalCustom.some(name => name.startsWith('.debug_'))) {
    throw new Error(`default output contains DWARF: ${normalCustom}`);
  }
  if (developmentCustom.some(name => name.startsWith('.debug_'))) {
    throw new Error(`runtime output contains inline DWARF: ${developmentCustom}`);
  }
  for (const name of ['.debug_abbrev', '.debug_info', '.debug_line']) {
    if (!debugCustom.includes(name)) throw new Error(`debug companion is missing ${name}`);
  }
  if (!normalCustom.includes('name') || !developmentCustom.includes('name') || !debugCustom.includes('name')) {
    throw new Error('runtime outputs and the debugger snapshot must retain Wasm names');
  }
  const referenceSection = developmentSections.find(item => item.name === 'external_debug_info');
  if (!referenceSection) throw new Error('runtime output is missing external_debug_info');
  const referenceState = { offset: referenceSection.payloadStart };
  const debugUrl = readName(development, referenceState);
  if (debugUrl !== 'compiler_dwarf_probe.debug.wasm') {
    throw new Error(`unexpected external debug URL: ${debugUrl}`);
  }
  const syntheticFunction = findExportedFunction(
    development,
    developmentSections,
    name => name.startsWith('__waluau_new_record_'),
  );
  return {
    defaultBytes: normal.byteLength,
    developmentRuntimeBytes: development.byteLength,
    runtimeReferenceOverheadBytes: development.byteLength - normal.byteLength,
    runtimeReferenceOverheadPercent: Number(
      (((development.byteLength - normal.byteLength) / normal.byteLength) * 100).toFixed(1),
    ),
    debugCompanionBytes: debug.byteLength,
    debugUrl,
    defaultCustomSections: normalCustom,
    developmentCustomSections: developmentCustom,
    debugCustomSections: debugCustom,
    syntheticFunction: syntheticFunction.name,
    syntheticOffset: functionInstructionOffset(
      development,
      developmentSections,
      syntheticFunction.index,
    ),
  };
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

async function startServer(extension) {
  const child = spawn(process.execPath, [join(fixtureDir, 'serve-probe.mjs'), extension], {
    cwd: repoRoot,
    env: { ...process.env, PORT: '0' },
    stdio: ['ignore', 'pipe', 'inherit'],
  });
  let output = '';
  const origin = await new Promise((resolveOrigin, reject) => {
    const timeout = setTimeout(() => reject(new Error('probe server did not start')), 10_000);
    child.once('error', reject);
    child.stdout.on('data', chunk => {
      output += chunk;
      const match = output.match(/runtime: (http:\/\/127\.0\.0\.1:\d+)\//);
      if (match) {
        clearTimeout(timeout);
        resolveOrigin(match[1]);
      }
    });
  });
  return { child, origin };
}

async function verifyExtensionWorker(chromium, options, origin, syntheticOffset) {
  const profile = mkdtempSync(join(tmpdir(), 'waluau-dwarf-worker-'));
  let context;
  try {
    context = await chromium.launchPersistentContext(profile, {
      executablePath: options.chrome,
      headless: !options.headed,
    });
    const page = context.pages()[0] ?? await context.newPage();
    const parameters = new URLSearchParams({
      module: 'compiler_dwarf_probe.wasm',
      symbols: 'compiler_dwarf_probe.debug.wasm',
      syntheticOffset: String(syntheticOffset),
    });
    parameters.append('source', 'compiler_probe_main.walu');
    parameters.append('source', 'compiler_probe_helper.walu');
    await page.goto(`${origin}/extension/extension-harness.html?${parameters}`);
    await page.waitForFunction(() => globalThis.extensionProbeResult, undefined, { timeout: 30_000 });
    const result = await page.evaluate(() => globalThis.extensionProbeResult);
    for (const source of ['compiler_probe_main.walu', 'compiler_probe_helper.walu']) {
      const sourceResult = result.sourceResults[source];
      assert(sourceResult, `extension worker did not discover ${source}`);
      assert(sourceResult.mappedLinesOneBased.length > 0, `${source} has no mapped lines`);
      assert(
        Object.values(sourceResult.rawRanges).flat().length > 0,
        `${source} has no reverse source-to-Wasm mapping`,
      );
    }
    assert(result.firstRawMapping.length > 0, 'raw-to-source mapping is empty');
    assert(result.syntheticSourceMapping.length === 0, 'synthetic helper gained a source mapping');
    assert(
      !result.syntheticFunction?.frames || result.syntheticFunction.frames.length === 0,
      'synthetic helper gained a DWARF function frame',
    );
    return result;
  } finally {
    if (context) await context.close();
    rmSync(profile, { recursive: true, force: true });
  }
}

async function verifyRuntimePageDoesNotLoadDebug(chromium, options, origin) {
  const browser = await chromium.launch({
    executablePath: options.chrome,
    headless: !options.headed,
  });
  try {
    const page = await browser.newPage();
    const requests = [];
    page.on('request', request => requests.push(new URL(request.url()).pathname));
    await page.goto(`${origin}/fixture/compiler-probe.html`);
    await page.waitForFunction(() => globalThis.dwarfProbe);
    assert(
      requests.includes('/fixture/compiler_dwarf_probe.wasm'),
      `runtime module was not requested: ${requests}`,
    );
    assert(
      !requests.includes('/fixture/compiler_dwarf_probe.debug.wasm'),
      `ordinary page load fetched external DWARF: ${requests}`,
    );
    return { requests };
  } finally {
    await browser.close();
  }
}

async function attachDevTools(context, page) {
  const root = await context.newCDPSession(page);
  let targetInfo;
  for (let attempt = 0; attempt < 100; attempt += 1) {
    const { targetInfos } = await root.send('Target.getTargets');
    const inspected = targetInfos.find(target => target.type === 'page' && target.url.includes('compiler-probe.html'));
    targetInfo = targetInfos.find(target =>
      target.url.startsWith('devtools://') && inspected && target.openerId === inspected.targetId)
      ?? targetInfos.find(target => target.url.startsWith('devtools://'));
    if (targetInfo) break;
    await page.waitForTimeout(100);
  }
  if (!targetInfo) {
    throw new Error(
      'DevTools frontend target not found. Use Chrome for Testing; branded Chrome may ignore --load-extension.',
    );
  }
  const { sessionId } = await root.send('Target.attachToTarget', {
    targetId: targetInfo.targetId,
    flatten: false,
  });
  let nextId = 1;
  const pending = new Map();
  root.on('Target.receivedMessageFromTarget', event => {
    if (event.sessionId !== sessionId) return;
    const message = JSON.parse(event.message);
    const waiter = pending.get(message.id);
    if (!waiter) return;
    pending.delete(message.id);
    if (message.error) waiter.reject(new Error(JSON.stringify(message.error)));
    else waiter.resolve(message.result);
  });
  const send = (method, params = {}) => new Promise((resolveResult, reject) => {
    const id = nextId++;
    pending.set(id, { resolve: resolveResult, reject });
    root.send('Target.sendMessageToTarget', {
      sessionId,
      message: JSON.stringify({ id, method, params }),
    }).catch(reject);
  });
  await send('Runtime.enable');
  return async expression => {
    const result = await send('Runtime.evaluate', {
      expression,
      awaitPromise: true,
      returnByValue: true,
    });
    if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
    return result.result.value;
  };
}

async function verifyDevToolsUi(chromium, options, origin) {
  const profile = mkdtempSync(join(tmpdir(), 'waluau-dwarf-ui-'));
  let context;
  try {
    context = await chromium.launchPersistentContext(profile, {
      executablePath: options.chrome,
      headless: !options.headed,
      args: [
        `--disable-extensions-except=${options.extension}`,
        `--load-extension=${options.extension}`,
        '--auto-open-devtools-for-tabs',
      ],
    });
    const page = context.pages()[0] ?? await context.newPage();
    const consoleErrors = [];
    const requests = [];
    const parsedWasm = [];
    page.on('request', request => requests.push(request.url()));
    page.on('console', message => {
      if (message.type() === 'error') consoleErrors.push(message.text());
    });
    const pageDebugger = await context.newCDPSession(page);
    pageDebugger.on('Debugger.scriptParsed', event => {
      if (event.url.startsWith('wasm://')) {
        parsedWasm.push({ url: event.url, debugSymbols: event.debugSymbols });
      }
    });
    await pageDebugger.send('Debugger.enable');
    await page.goto(`${origin}/fixture/compiler-probe.html`);
    await page.waitForFunction(() => globalThis.dwarfProbe);
    await page.waitForTimeout(3_000);
    await page.reload();
    await page.waitForFunction(() => globalThis.dwarfProbe);
    const evaluate = await attachDevTools(context, page);

    const installed = await evaluate(`(async () => {
      const Common = await import('./core/common/common.js');
      const Workspace = await import('./models/workspace/workspace.js');
      const Breakpoints = await import('./models/breakpoints/breakpoints.js');
      const workspace = Workspace.Workspace.WorkspaceImpl.instance();
      let helper;
      for (let attempt = 0; attempt < 200; attempt += 1) {
        helper = workspace.uiSourceCodes().find(source => source.url().endsWith('/compiler_probe_helper.walu'));
        if (helper) break;
        await new Promise(resolve => setTimeout(resolve, 100));
      }
      if (!helper) return {
        sources: workspace.uiSourceCodes().map(source => source.url()),
        devToolsConsole: Common.Console.Console.instance().messages().map(message => ({
          text: message.text,
          level: message.level,
        })),
      };
      const manager = Breakpoints.BreakpointManager.BreakpointManager.instance();
      const breakpoint = await manager.setBreakpoint(helper, 3, undefined, '', true, false, 'OTHER');
      await breakpoint.updateBreakpoint();
      globalThis.__waluauUiProbe = { helper, manager, breakpoint };
      return {
        sourceUrl: helper.url(),
        breakpointLine: breakpoint.lineNumber(),
        breakpointColumn: breakpoint.columnNumber(),
        bound: breakpoint.bound(),
      };
    })()`);
    assert(
      !installed.sources,
      `authored helper source not found: ${JSON.stringify({
        installed,
        requests,
        parsedWasm,
      })}`,
    );
    assert(installed.bound, 'line breakpoint did not bind');
    assert(installed.breakpointColumn > 0, 'breakpoint did not normalize to an authored column');

    await page.evaluate(() => setTimeout(() => document.querySelector('#run').click(), 0));
    const mappedCall = await evaluate(`(async () => {
      const SDK = await import('./core/sdk/sdk.js');
      const Bindings = await import('./models/bindings/bindings.js');
      const debuggerModel = SDK.TargetManager.TargetManager.instance().models(SDK.DebuggerModel.DebuggerModel)[0];
      for (let attempt = 0; attempt < 200; attempt += 1) {
        if (debuggerModel.debuggerPausedDetails()) break;
        await new Promise(resolve => setTimeout(resolve, 100));
      }
      if (!debuggerModel.debuggerPausedDetails()) throw new Error('debugger did not pause');
      globalThis.__waluauUiProbe.debuggerModel = debuggerModel;
      const binding = Bindings.DebuggerWorkspaceBinding.DebuggerWorkspaceBinding.instance();
      const frames = [];
      for (const frame of debuggerModel.debuggerPausedDetails().callFrames.slice(0, 6)) {
        const raw = frame.location();
        const ui = await binding.rawLocationToUILocation(raw);
        frames.push({
          functionName: frame.functionName,
          raw: { lineNumber: raw.lineNumber, columnNumber: raw.columnNumber },
          ui: ui ? { url: ui.uiSourceCode.url(), lineNumber: ui.lineNumber, columnNumber: ui.columnNumber } : null,
        });
      }
      return frames;
    })()`);
    assert(
      mappedCall.some(frame => frame.ui?.url.endsWith('/compiler_probe_helper.walu') && frame.ui.lineNumber === 3),
      `paused helper frame was not mapped: ${JSON.stringify(mappedCall)}`,
    );
    assert(
      mappedCall.some(frame => frame.ui?.url.endsWith('/compiler_probe_main.walu') && frame.ui.lineNumber === 3),
      `paused caller frame was not mapped across files: ${JSON.stringify(mappedCall)}`,
    );

    const stepped = await evaluate(`(async () => {
      const Bindings = await import('./models/bindings/bindings.js');
      const debuggerModel = globalThis.__waluauUiProbe.debuggerModel;
      const binding = Bindings.DebuggerWorkspaceBinding.DebuggerWorkspaceBinding.instance();
      let previousOffset = debuggerModel.debuggerPausedDetails().callFrames[0].location().columnNumber;
      for (let step = 0; step < 20; step += 1) {
        await debuggerModel.stepOver();
        for (let attempt = 0; attempt < 200; attempt += 1) {
          const frame = debuggerModel.debuggerPausedDetails()?.callFrames?.[0];
          if (frame && frame.location().columnNumber !== previousOffset) {
            previousOffset = frame.location().columnNumber;
            const ui = await binding.rawLocationToUILocation(frame.location());
            if (ui?.lineNumber !== 3) {
              return {
                steps: step + 1,
                url: ui.uiSourceCode.url(),
                lineNumber: ui.lineNumber,
                columnNumber: ui.columnNumber,
              };
            }
            break;
          }
          await new Promise(resolve => setTimeout(resolve, 100));
        }
      }
      throw new Error('step-over did not reach another source location');
    })()`);
    assert(
      stepped?.url.endsWith('/compiler_probe_helper.walu') && stepped.lineNumber === 4,
      `step-over did not reach helper line 5: ${JSON.stringify(stepped)}`,
    );
    await evaluate('globalThis.__waluauUiProbe.debuggerModel.resume()');

    await page.click('#console-error');
    const caughtError = await page.evaluate(() => ({
      constructor: globalThis.lastProbeError.constructor.name,
      stack: globalThis.lastProbeError.stack ?? null,
      text: String(globalThis.lastProbeError),
    }));
    if (caughtError.stack !== null) {
      assert(
        /wasm-function\[\d+\]:0x[0-9a-f]+/i.test(caughtError.stack),
        `Error.stack lost raw Wasm offsets: ${caughtError.stack}`,
      );
      assert(!caughtError.stack.includes('.walu:'), 'Error.stack was unexpectedly rewritten through DWARF');
    }
    assert(consoleErrors.length > 0, 'console.error did not observe the caught exception');
    assert(
      consoleErrors.every(message => !message.includes('.walu:')),
      `console.error was unexpectedly rewritten through DWARF: ${consoleErrors}`,
    );

    const exceptionBreakpoint = await evaluate(`(async () => {
      const breakpoint = await globalThis.__waluauUiProbe.manager.setBreakpoint(
        globalThis.__waluauUiProbe.helper, 8, undefined, '', true, false, 'OTHER');
      await breakpoint.updateBreakpoint();
      return { line: breakpoint.lineNumber(), column: breakpoint.columnNumber(), bound: breakpoint.bound() };
    })()`);
    assert(exceptionBreakpoint.bound, 'exception-path breakpoint did not bind');
    const pageErrorPromise = page.waitForEvent('pageerror', { timeout: 30_000 });
    await page.evaluate(() => setTimeout(() => document.querySelector('#uncaught').click(), 0));
    const exceptionCall = await evaluate(`(async () => {
      const Bindings = await import('./models/bindings/bindings.js');
      const debuggerModel = globalThis.__waluauUiProbe.debuggerModel;
      for (let attempt = 0; attempt < 200; attempt += 1) {
        const details = debuggerModel.debuggerPausedDetails();
        if (details?.callFrames?.length) {
          const binding = Bindings.DebuggerWorkspaceBinding.DebuggerWorkspaceBinding.instance();
          const frames = [];
          for (const frame of details.callFrames.slice(0, 6)) {
            const ui = await binding.rawLocationToUILocation(frame.location());
            frames.push({
              functionName: frame.functionName,
              ui: ui ? { url: ui.uiSourceCode.url(), lineNumber: ui.lineNumber, columnNumber: ui.columnNumber } : null,
            });
          }
          return frames;
        }
        await new Promise(resolve => setTimeout(resolve, 100));
      }
      throw new Error('exception path did not pause');
    })()`);
    assert(
      exceptionCall.some(frame => frame.ui?.url.endsWith('/compiler_probe_helper.walu') && frame.ui.lineNumber === 8),
      `exception helper frame was not mapped: ${JSON.stringify(exceptionCall)}`,
    );
    assert(
      exceptionCall.some(frame => frame.ui?.url.endsWith('/compiler_probe_main.walu') && frame.ui.lineNumber === 7),
      `exception caller frame was not mapped: ${JSON.stringify(exceptionCall)}`,
    );
    await evaluate('globalThis.__waluauUiProbe.debuggerModel.resume()');
    const uncaught = await pageErrorPromise;
    const uncaughtStack = uncaught.stack ?? null;
    if (uncaughtStack) {
      assert(
        /wasm-function\[\d+\]:0x[0-9a-f]+/i.test(uncaughtStack),
        `uncaught error lost raw Wasm offsets: ${uncaughtStack}`,
      );
      assert(!uncaughtStack.includes('.walu:'), 'uncaught error text was unexpectedly rewritten through DWARF');
    }

    return {
      installed,
      mappedCall,
      stepped,
      caughtError,
      consoleErrorObserved: true,
      exceptionBreakpoint,
      exceptionCall,
      uncaughtStack,
    };
  } finally {
    if (context) await context.close();
    rmSync(profile, { recursive: true, force: true });
  }
}

const options = parseArguments(process.argv.slice(2));
const { chromium } = loadPlaywright();
const scratch = mkdtempSync(join(tmpdir(), 'waluau-dwarf-artifacts-'));
const defaultOutput = join(scratch, 'compiler_default_probe.wasm');
const developmentOutput = join(fixtureDir, 'compiler_dwarf_probe.wasm');
const debugOutput = join(fixtureDir, 'compiler_dwarf_probe.debug.wasm');
const source = join(fixtureDir, 'compiler_probe_main.walu');
let server;

try {
  run('cargo', ['run', '-q', '-p', 'waluau-cli', '--', source, '-o', defaultOutput]);
  run('cargo', [
    'run', '-q', '-p', 'waluau-cli', '--', source,
    '-o', developmentOutput, '--emit-js', '--development-dwarf',
  ]);
  const artifactInspection = inspectArtifacts(defaultOutput, developmentOutput, debugOutput);
  server = await startServer(options.extension);
  const runtime = await verifyRuntimePageDoesNotLoadDebug(chromium, options, server.origin);
  const worker = await verifyExtensionWorker(
    chromium,
    options,
    server.origin,
    artifactInspection.syntheticOffset,
  );
  const devTools = await verifyDevToolsUi(chromium, options, server.origin);
  process.stdout.write(`${JSON.stringify({ artifactInspection, runtime, worker, devTools }, null, 2)}\n`);
} finally {
  if (server?.child) server.child.kill('SIGTERM');
  rmSync(scratch, { recursive: true, force: true });
}
