import { useCallback, useEffect, useRef, useState } from 'react';
import {
  WALUAU_STRING_CONSTANTS_MODULE,
  buildWaluauImports,
  usesDomImports,
  classifyWasmModuleError,
} from '../utils/wasm.js';

const REPL_PATH = '/repl.walu';

function stringifyError(err) {
  return typeof err === 'string' ? err : err?.message || String(err);
}

/**
 * REPL execution model: accumulate-and-recompile.
 *
 * The compiler produces a fresh, stateless wasm module on every call, so there
 * is no incremental/persistent instance to feed lines into. Instead we keep a
 * buffer of every accepted cell, concatenate it with the new cell, recompile
 * the whole program, and run it. Because the program is deterministic and we
 * only ever append, the print output of prior cells reproduces identically as a
 * prefix — so the *new* output is just the tail beyond what the previous run
 * produced. A cell that fails to compile or instantiate is not added to the
 * buffer, leaving the session state intact.
 *
 * The session can optionally be *seeded* from the editor program (see `seed`).
 * Seeding snapshots the editor's file map and entry path; subsequent cells are
 * appended to the entry file, so the script's top-level definitions and imports
 * are in scope. The snapshot is taken once, so later editor edits do not
 * retroactively change the running session.
 */
export default function useWaluauRepl() {
  const [ready, setReady] = useState(false);
  const [loadError, setLoadError] = useState('');
  const [cells, setCells] = useState([]); // { input, output: string[], error, ok, isSeed? }
  const [busy, setBusy] = useState(false);

  const compileMultiRef = useRef(null);
  const acceptedRef = useRef([]); // accepted cell sources (compile + run cleanly)
  const prevLogCountRef = useRef(0); // total prints produced by the last good run
  const baseFilesRef = useRef({}); // editor file snapshot seeding the session
  const baseEntryRef = useRef(REPL_PATH); // entry path that cells are appended to
  const autoSeedDoneRef = useRef(false); // first-open auto-seed guard

  useEffect(() => {
    let cancelled = false;
    import('../waluau-wasm/waluau_wasm.js')
      .then(async (module) => {
        await module.default();
        if (cancelled) return;
        compileMultiRef.current = module.compile_multi;
        setReady(true);
      })
      .catch((err) => {
        if (cancelled) return;
        setLoadError(`Failed to load WASM compiler: ${err.message}`);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Build the files map for a run: the editor snapshot, with the entry file
  // extended by all accepted cells plus any extra (not-yet-accepted) sources.
  const buildFilesMap = useCallback((extraCells) => {
    const entry = baseEntryRef.current;
    const baseEntrySource = baseFilesRef.current[entry] ?? '';
    const entrySource = [baseEntrySource, ...acceptedRef.current, ...extraCells]
      .filter((part) => part.trim() !== '')
      .join('\n\n');
    return { files: { ...baseFilesRef.current, [entry]: entrySource }, entry };
  }, []);

  // Compile + instantiate a program, capturing every print call. Never throws.
  const runProgram = useCallback(async (filesMap, entry) => {
    const compileMulti = compileMultiRef.current;
    if (!compileMulti) return { ok: false, error: 'Compiler not ready' };

    let result;
    try {
      result = compileMulti(filesMap, entry);
    } catch (err) {
      return { ok: false, error: stringifyError(err) };
    }

    const logs = [];
    let phase = 'compile';
    try {
      const wasmBuffer = new Uint8Array(result.wasm);
      const wasmModule = await WebAssembly.compile(wasmBuffer, {
        builtins: ['js-string'],
        importedStringConstants: WALUAU_STRING_CONSTANTS_MODULE,
      });

      phase = 'inspect';
      if (usesDomImports(wasmModule, wasmBuffer)) {
        return {
          ok: false,
          error: 'This program uses DOM imports, which the REPL does not host yet. Use the Run tab for DOM presets.',
        };
      }

      phase = 'imports';
      let instanceExports = null;
      const imports = buildWaluauImports(wasmModule, (msg) => logs.push(msg), {
        wasmBytes: wasmBuffer,
        getWasmExports: () => instanceExports,
      });

      phase = 'instantiate';
      const instance = await WebAssembly.instantiate(wasmModule, imports);
      instanceExports = instance.exports;
    } catch (err) {
      return {
        ok: false,
        error: classifyWasmModuleError(err, phase, Boolean(result.requiresWasmGc)),
      };
    }

    // Let any synchronous coroutine/promise continuations flush their prints.
    await Promise.resolve();
    return { ok: true, logs };
  }, []);

  const evaluate = useCallback(async (rawInput) => {
    const input = rawInput.replace(/\s+$/, '');
    if (!input.trim() || !compileMultiRef.current) return;

    setBusy(true);
    try {
      const { files, entry } = buildFilesMap([input]);
      const res = await runProgram(files, entry);
      if (!res.ok) {
        setCells((prev) => [...prev, { input, output: [], error: res.error, ok: false }]);
        return;
      }
      const newOutput = res.logs.slice(prevLogCountRef.current);
      acceptedRef.current = [...acceptedRef.current, input];
      prevLogCountRef.current = res.logs.length;
      setCells((prev) => [...prev, { input, output: newOutput, error: null, ok: true }]);
    } finally {
      setBusy(false);
    }
  }, [buildFilesMap, runProgram]);

  // Seed the session from the editor program. Resets any existing session,
  // snapshots the file map, runs the script once to surface its output, and
  // establishes the print baseline so later cells show only their own output.
  const seed = useCallback(async (editorFiles, entryPath) => {
    if (!compileMultiRef.current) return;
    const entry = entryPath ?? REPL_PATH;
    const label = `loaded editor script (${entry.replace(/^\//, '')})`;

    setBusy(true);
    try {
      acceptedRef.current = [];
      prevLogCountRef.current = 0;
      baseFilesRef.current = { ...editorFiles };
      baseEntryRef.current = entry;

      const { files, entry: runEntry } = buildFilesMap([]);
      const res = await runProgram(files, runEntry);
      if (!res.ok) {
        // The script does not run on its own; fall back to a blank session so
        // the REPL stays usable, and surface the failure.
        baseFilesRef.current = {};
        baseEntryRef.current = REPL_PATH;
        setCells([{ input: label, output: [], error: res.error, ok: false, isSeed: true }]);
        return;
      }
      prevLogCountRef.current = res.logs.length;
      setCells([{ input: label, output: res.logs, error: null, ok: true, isSeed: true }]);
    } finally {
      setBusy(false);
    }
  }, [buildFilesMap, runProgram]);

  // Seed automatically the first time the REPL is opened in this session.
  const maybeAutoSeed = useCallback((editorFiles, entryPath) => {
    if (autoSeedDoneRef.current) return;
    autoSeedDoneRef.current = true;
    seed(editorFiles, entryPath);
  }, [seed]);

  const reset = useCallback(() => {
    acceptedRef.current = [];
    prevLogCountRef.current = 0;
    baseFilesRef.current = {};
    baseEntryRef.current = REPL_PATH;
    setCells([]);
  }, []);

  return { ready, loadError, cells, busy, evaluate, reset, seed, maybeAutoSeed };
}
