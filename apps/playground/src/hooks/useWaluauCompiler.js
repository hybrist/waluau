import { useState, useEffect, useRef, useCallback, useMemo } from 'react';
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
import { getWaluauCompilerClient } from '../utils/waluauCompilerClient.js';
import { withTypedAssetModule } from '../utils/typedAssets.js';

export default function useWaluauCompiler({ files, entryFile, assetManifest = null }) {
  const compilerFiles = useMemo(
    () => withTypedAssetModule(files, assetManifest),
    [files, assetManifest],
  );
  const [status, setStatus] = useState('loading'); // 'loading', 'analyzing', 'ready', 'success', 'error'
  const [loadErrorMsg, setLoadErrorMsg] = useState('');
  const [output, setOutput] = useState('');
  const [errorMsg, setErrorMsg] = useState('');
  const [diagnostics, setDiagnostics] = useState(null);
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
  const compilerClientRef = useRef(null);
  const completedAnalysisRef = useRef(false);

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

  // The worker owns both the compiler and language-server Wasm objects. One
  // combined request links and typechecks each snapshot once, returning both
  // compiler output and structured diagnostics. The client coalesces rapid
  // edits while an older snapshot is still running.
  useEffect(() => {
    let active = true;
    const client = compilerClientRef.current ?? getWaluauCompilerClient();
    compilerClientRef.current = client;
    setStatus(completedAnalysisRef.current ? 'analyzing' : 'loading');

    client.analyzeProject(compilerFiles, entryFile)
      .then((result) => {
        if (!active || result == null) return;
        completedAnalysisRef.current = true;
        setOutput(result.output ?? '');
        setErrorMsg(result.errorMsg ?? '');
        setDiagnostics(
          result.diagnostics instanceof Map
            ? result.diagnostics
            : new Map(Object.entries(result.diagnostics ?? {})),
        );
        setStatus(result.errorMsg ? 'error' : result.output ? 'success' : 'ready');
        setLoadErrorMsg('');
      })
      .catch((err) => {
        if (!active) return;
        console.error('Compiler worker error:', err);
        const message = `Compiler worker failed: ${err.message}`;
        setStatus('error');
        setLoadErrorMsg(message);
        setErrorMsg(message);
        setOutput('');
        setDiagnostics(null);
      });

    return () => {
      active = false;
    };
  }, [compilerFiles, entryFile]);

  // Monaco accepts promises from language providers, so position requests can
  // cross the worker seam without blocking the UI. The live document keeps a
  // request current when it races ahead of React's files-state update.
  const sendLspRequest = useCallback((method, params, document = null) => {
    const client = compilerClientRef.current;
    if (!client) return Promise.resolve(null);
    return client.lspRequest(method, params, document).catch((error) => {
      console.warn('Waluau language server request failed:', error);
      return null;
    });
  }, []);

  const outputIr = typeof output === 'object' ? output.ir : '';
  const outputWat = typeof output === 'object' ? output.wat : '';
  const outputWasmBytes = typeof output === 'object' ? output.wasm : null;
  const requiresWasmGc = typeof output === 'object' ? Boolean(output.requiresWasmGc) : false;
  const displayStatus = status;

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
        const generatedMain = wasmExports.find(func => func.name === WALUAU_MAIN_EXPORT);
        const list = wasmExports
          .filter(func => !func.name.startsWith('__waluau'))
          // Hide the compatibility alias for generated initialization, but
          // keep an authored `export function main` whose Wasm identity is
          // distinct from the `__waluau_main` runtime entry.
          .filter(func => !(func.name === 'main' && func.index === generatedMain?.index))
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
    sendLspRequest,
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
