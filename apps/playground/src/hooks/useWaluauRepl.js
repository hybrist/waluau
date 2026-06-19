import { useCallback, useEffect, useRef, useState } from 'react';
import {
  WALUAU_STRING_CONSTANTS_MODULE,
  buildWaluauImports,
  usesDomImports,
  classifyWasmInstantiationError,
} from '../utils/wasm.js';

const REPL_PATH = '/repl.walu';

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
 */
export default function useWaluauRepl() {
  const [ready, setReady] = useState(false);
  const [loadError, setLoadError] = useState('');
  const [cells, setCells] = useState([]); // { input, output: string[], error, ok }
  const [busy, setBusy] = useState(false);

  const compileMultiRef = useRef(null);
  const acceptedRef = useRef([]); // accepted cell sources (compile + run cleanly)
  const prevLogCountRef = useRef(0); // total prints produced by the last good run

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

  const reset = useCallback(() => {
    acceptedRef.current = [];
    prevLogCountRef.current = 0;
    setCells([]);
  }, []);

  const evaluate = useCallback(async (rawInput) => {
    const input = rawInput.replace(/\s+$/, '');
    if (!input.trim()) return;

    const compileMulti = compileMultiRef.current;
    if (!compileMulti) return;

    setBusy(true);
    try {
      const combined = [...acceptedRef.current, input].join('\n\n');

      // 1. Compile the whole accumulated program.
      let result;
      try {
        result = compileMulti({ [REPL_PATH]: combined }, REPL_PATH);
      } catch (err) {
        const message = typeof err === 'string' ? err : err?.message || String(err);
        setCells((prev) => [...prev, { input, output: [], error: message, ok: false }]);
        return;
      }

      // 2. Compile + instantiate the wasm, capturing every print call.
      const wasmBuffer = new Uint8Array(result.wasm);
      const wasmModule = await WebAssembly.compile(wasmBuffer, {
        builtins: ['js-string'],
        importedStringConstants: WALUAU_STRING_CONSTANTS_MODULE,
      });

      if (usesDomImports(wasmModule)) {
        setCells((prev) => [
          ...prev,
          {
            input,
            output: [],
            error: 'This snippet uses DOM imports, which the REPL does not host yet. Use the Run tab for DOM presets.',
            ok: false,
          },
        ]);
        return;
      }

      const logs = [];
      let instanceExports = null;
      const imports = buildWaluauImports(wasmModule, (msg) => logs.push(msg), {
        getWasmExports: () => instanceExports,
      });

      try {
        const instance = await WebAssembly.instantiate(wasmModule, imports);
        instanceExports = instance.exports;
      } catch (err) {
        const requiresWasmGc = Boolean(result.requiresWasmGc);
        setCells((prev) => [
          ...prev,
          { input, output: [], error: classifyWasmInstantiationError(err, requiresWasmGc), ok: false },
        ]);
        return;
      }

      // Let any synchronous coroutine/promise continuations flush their prints.
      await Promise.resolve();

      // 3. Show only the output beyond what the previous good run produced.
      const newOutput = logs.slice(prevLogCountRef.current);
      acceptedRef.current = [...acceptedRef.current, input];
      prevLogCountRef.current = logs.length;
      setCells((prev) => [...prev, { input, output: newOutput, error: null, ok: true }]);
    } finally {
      setBusy(false);
    }
  }, []);

  return { ready, loadError, cells, busy, evaluate, reset };
}
