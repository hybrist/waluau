import { useState, useEffect, useMemo, useRef, useCallback } from 'react';
import {
  WALUAU_STRING_CONSTANTS_MODULE,
  WALUAU_MAIN_EXPORT,
  buildWaluauImports,
  getWasmExports,
  getDefaultParamValue,
  executeCall,
  classifyWasmModuleError,
  usesDomImports,
  cleanupDomEventListeners,
  cancelPendingAnimationFrames,
} from '../utils/wasm.js';

export default function useWaluauCompiler({ files, entryFile, assetManifest = null }) {
  const [status, setStatus] = useState('loading'); // 'loading', 'ready', 'success', 'error'
  const [loadErrorMsg, setLoadErrorMsg] = useState('');
  const [compilerReady, setCompilerReady] = useState(false);
  const [compileSource, setCompileSource] = useState(null);
  const [runInstance, setRunInstance] = useState(null);
  const [runError, setRunError] = useState(null);
  const [exportsList, setExportsList] = useState([]);
  const [initLogs, setInitLogs] = useState([]);
  const [funcInputs, setFuncInputs] = useState({});
  const [autoRun, setAutoRunEnabled] = useState(true);
  const [autoResults, setAutoResults] = useState({});
  const [manualResults, setManualResults] = useState({});
  const [usesDomOutput, setUsesDomOutput] = useState(false);
  const [domMountVersion, setDomMountVersion] = useState(0);
  const domOutputRootRef = useRef(null);
  const autoExecutionRef = useRef(new Map());
  const languageServerRef = useRef(null);
  // Documents synced to the language server: path -> { version, text }.
  const syncedDocumentsRef = useRef(new Map());
  const lastDiagnosticsRef = useRef(null);

  const setDomOutputRoot = useCallback((node) => {
    if (domOutputRootRef.current === node) return;
    domOutputRootRef.current = node;
    setDomMountVersion((version) => version + 1);
  }, []);

  const setAutoRun = useCallback((enabled) => {
    autoExecutionRef.current.clear();
    setAutoResults({});
    setAutoRunEnabled(enabled);
  }, []);

  // Load compiler module
  useEffect(() => {
    let cancelled = false;

    import('../waluau-wasm/waluau_wasm.js')
      .then(async (module) => {
        await module.default();
        if (cancelled) {
          return;
        }
        setCompileSource(() => module.compile_multi);
        if (typeof module.WaluauLanguageServer === 'function') {
          languageServerRef.current = new module.WaluauLanguageServer();
        }
        setCompilerReady(true);
        setStatus('ready');
        setLoadErrorMsg('');
      })
      .catch((err) => {
        if (cancelled) {
          return;
        }
        console.error('WASM load error:', err);
        setStatus('error');
        setLoadErrorMsg(`Failed to load WASM compiler: ${err.message}`);
      });

    return () => {
      cancelled = true;
    };
  }, []);

  // Run compilation
  const compilation = useMemo(() => {
    if (!compilerReady || !compileSource) {
      return {
        output: '',
        errorMsg: status === 'error' ? loadErrorMsg : '',
      };
    }

    try {
      const parsed = compileSource(files, entryFile);
      return {
        output: parsed,
        errorMsg: '',
      };
    } catch (err) {
      const message = typeof err === 'string' ? err : err?.message || String(err);
      return {
        output: '',
        errorMsg: message,
      };
    }
  }, [files, entryFile, compileSource, compilerReady, loadErrorMsg, status]);

  // Client-side language server: sync changed documents over the LSP
  // lifecycle and collect per-file publishDiagnostics. Null when the wasm
  // build predates the language server (markers then fall back to errorMsg).
  const diagnostics = useMemo(() => {
    const server = languageServerRef.current;
    if (!server || !compilerReady) return null;
    const collected = new Map();
    let sentAny = false;
    const send = (message) => {
      sentAny = true;
      for (const outgoing of server.handleMessage(JSON.stringify(message))) {
        const parsed = JSON.parse(outgoing);
        if (parsed.method === 'textDocument/publishDiagnostics') {
          collected.set(parsed.params.uri, parsed.params.diagnostics);
        }
      }
    };
    const synced = syncedDocumentsRef.current;
    for (const path of [...synced.keys()]) {
      if (!(path in files)) {
        synced.delete(path);
        send({
          jsonrpc: '2.0',
          method: 'textDocument/didClose',
          params: { textDocument: { uri: `file://${path}` } },
        });
      }
    }
    for (const [path, text] of Object.entries(files)) {
      const uri = `file://${path}`;
      const known = synced.get(path);
      if (known == null) {
        synced.set(path, { version: 1, text });
        send({
          jsonrpc: '2.0',
          method: 'textDocument/didOpen',
          params: { textDocument: { uri, languageId: 'waluau', version: 1, text } },
        });
      } else if (known.text !== text) {
        const version = known.version + 1;
        synced.set(path, { version, text });
        send({
          jsonrpc: '2.0',
          method: 'textDocument/didChange',
          params: {
            textDocument: { uri, version },
            contentChanges: [{ text }],
          },
        });
      }
    }
    // Each server response batch reflects the complete diagnostic state; if
    // nothing changed (memo re-ran for another reason), keep the last state.
    if (!sentAny) return lastDiagnosticsRef.current;
    lastDiagnosticsRef.current = collected;
    return collected;
  }, [files, compilerReady]);

  const output = compilation.output;
  const errorMsg = compilation.errorMsg;
  const outputIr = typeof output === 'object' ? output.ir : '';
  const outputWat = typeof output === 'object' ? output.wat : '';
  const outputWasmBytes = typeof output === 'object' ? output.wasm : null;
  const requiresWasmGc = typeof output === 'object' ? Boolean(output.requiresWasmGc) : false;
  const displayStatus = compilerReady
    ? errorMsg
      ? 'error'
      : output
        ? 'success'
        : 'ready'
    : status;

  // Sync runInstance, exportsList, inputs and results when Wasm changes
  useEffect(() => {
    let active = true;
    async function loadModule() {
      await Promise.resolve(); // Yield to prevent synchronous setState warnings
      if (!outputWasmBytes) {
        if (active) {
          setRunInstance(null);
          setRunError(null);
          setExportsList([]);
          autoExecutionRef.current.clear();
          setAutoResults({});
          setManualResults({});
          setInitLogs([]);
          setUsesDomOutput(false);
        }
        return;
      }
      if (active) {
        setRunInstance(null);
        setRunError(null);
        setExportsList([]);
        autoExecutionRef.current.clear();
        setAutoResults({});
        setManualResults({});
        setInitLogs([]);
      }
      const capturedInitLogs = [];
      const initLogger = (msg) => {
        capturedInitLogs.push(msg);
        // Update state on every message so that prints from async continuations
        // (coroutine resumes after a promise resolves) are visible in the UI.
        if (active) {
          setInitLogs([...capturedInitLogs]);
        }
      };
      let phase = 'compile';
      let moduleUsesDomOutput = false;
      try {
        const wasmBuffer = new Uint8Array(outputWasmBytes);
        const wasmModule = await WebAssembly.compile(wasmBuffer, {
          builtins: ['js-string'],
          importedStringConstants: WALUAU_STRING_CONSTANTS_MODULE,
        });

        phase = 'inspect';
        moduleUsesDomOutput = usesDomImports(wasmModule, wasmBuffer, output?.requiredImports);
        const richSigs = output?.signatures || {};
        const wasmExports = getWasmExports(wasmBuffer);
        const hasGeneratedMain = wasmExports.some(func => func.name === WALUAU_MAIN_EXPORT);
        const list = wasmExports
          .filter(func => !func.name.startsWith('__waluau'))
          .filter(func => !(hasGeneratedMain && func.name === 'main'))
          .map(func => {
            const richSig = (richSigs instanceof Map || (richSigs && typeof richSigs.get === 'function'))
              ? richSigs.get(func.name)
              : richSigs[func.name];
            return {
              ...func,
              richParams: richSig ? richSig.params : null,
              richReturns: richSig ? richSig.returns : null,
            };
          });
        if (active) {
          setUsesDomOutput(moduleUsesDomOutput);
          setExportsList(list);
          setFuncInputs(prev => {
            const next = { ...prev };
            let changed = false;
            for (const func of list) {
              if (!next[func.name] || next[func.name].length !== func.params.length) {
                const defaultVals = func.richParams
                  ? func.richParams.map(getDefaultParamValue)
                  : func.params.map(getDefaultParamValue);
                next[func.name] = defaultVals;
                changed = true;
              }
            }
            return changed ? next : prev;
          });
        }
        if (moduleUsesDomOutput && !domOutputRootRef.current) {
          return;
        }
        if (moduleUsesDomOutput) {
          cleanupDomEventListeners(domOutputRootRef.current.body);
          cancelPendingAnimationFrames(domOutputRootRef.current);
          domOutputRootRef.current.body.replaceChildren();
        }
        phase = 'imports';
        let instance;
        const imports = buildWaluauImports(wasmModule, initLogger, {
          wasmBytes: wasmBuffer,
          requiredImports: output?.requiredImports,
          bytesConstants: output?.bytesConstants,
          domOutputRoot: moduleUsesDomOutput ? domOutputRootRef.current : null,
          getWasmExports: () => instance.exports,
          gameServices: { assetManifest },
        });
        phase = 'instantiate';
        instance = await WebAssembly.instantiate(wasmModule, imports);
        phase = 'execute';
        instance.exports[WALUAU_MAIN_EXPORT]?.();

        if (active) {
          setRunInstance(instance);
          setRunError(null);
          autoExecutionRef.current.clear();
          setAutoResults({});
          setManualResults({});
          setInitLogs([...capturedInitLogs]);
        }
      } catch (err) {
        if (active) {
          console.error(`Generated WASM ${phase} failed:`, err);
          setRunInstance(null);
          setRunError(classifyWasmModuleError(err, phase, requiresWasmGc));
          setExportsList([]);
          autoExecutionRef.current.clear();
          setAutoResults({});
          setManualResults({});
          setInitLogs(capturedInitLogs);
          setUsesDomOutput(moduleUsesDomOutput);
        }
      }
    }

    loadModule();

    return () => {
      active = false;
    };
  }, [outputWasmBytes, requiresWasmGc, output, domMountVersion, assetManifest]);

  // Consumers request auto-run only for functions they render. The keyed cache
  // makes overlapping RunTab/InlineRunner requests and StrictMode effect replay
  // idempotent without executing unrelated or invisible exports.
  const requestAutoRun = useCallback((funcName, params, richParams, richReturns) => {
    if (!autoRun || !runInstance) return;

    const inputState = funcInputs[funcName];
    const previous = autoExecutionRef.current.get(funcName);
    if (
      previous?.instance === runInstance &&
      previous.inputState === inputState &&
      previous.params === params &&
      previous.richParams === richParams &&
      previous.richReturns === richReturns
    ) {
      return;
    }

    const inputs =
      inputState || (richParams || params).map(getDefaultParamValue);
    const record = {
      instance: runInstance,
      inputState,
      params,
      richParams,
      richReturns,
      result: executeCall(
        runInstance,
        funcName,
        params,
        richParams,
        richReturns,
        inputs,
        output?.tagIds
      ),
    };
    autoExecutionRef.current.set(funcName, record);
    queueMicrotask(() => {
      if (autoExecutionRef.current.get(funcName) !== record) return;
      setAutoResults((current) => ({ ...current, [funcName]: record }));
    });
  }, [autoRun, runInstance, funcInputs, output?.tagIds]);

  const handleInputChange = (funcName, paramIndex, value) => {
    setFuncInputs(prev => {
      const funcParams = prev[funcName] ? [...prev[funcName]] : [];
      funcParams[paramIndex] = value;
      return { ...prev, [funcName]: funcParams };
    });
  };

  const handleRecordFieldChange = (funcName, paramIndex, fieldPath, value) => {
    setFuncInputs(prev => {
      const funcParams = prev[funcName] ? [...prev[funcName]] : [];
      let current = { ...funcParams[paramIndex] };
      funcParams[paramIndex] = current;

      for (let i = 0; i < fieldPath.length - 1; i++) {
        const key = fieldPath[i];
        current[key] = { ...current[key] };
        current = current[key];
      }
      current[fieldPath[fieldPath.length - 1]] = value;

      return { ...prev, [funcName]: funcParams };
    });
  };

  const handleManualRun = (funcName, params, richParams, richReturns) => {
    const inputs = funcInputs[funcName] || (richParams || params).map(getDefaultParamValue);
    const res = executeCall(runInstance, funcName, params, richParams, richReturns, inputs, output?.tagIds);
    setManualResults(prev => ({ ...prev, [funcName]: res }));
  };

  const getResult = (funcName, params, richParams, richReturns) => {
    if (autoRun) {
      const record = autoResults[funcName];
      if (
        record?.instance === runInstance &&
        record.inputState === funcInputs[funcName] &&
        record.params === params &&
        record.richParams === richParams &&
        record.richReturns === richReturns
      ) {
        return record.result;
      }
      return { isIdle: true };
    } else {
      return manualResults[funcName] || { isIdle: true };
    }
  };

  return {
    status,
    loadErrorMsg,
    outputIr,
    outputWat,
    outputWasmBytes,
    displayStatus,
    errorMsg,
    diagnostics,
    runError,
    exportsList,
    initLogs,
    usesDomOutput,
    setDomOutputRoot,
    funcInputs,
    autoRun,
    setAutoRun,
    handleInputChange,
    handleRecordFieldChange,
    handleManualRun,
    requestAutoRun,
    getResult
  };
}
