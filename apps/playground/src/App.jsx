import { useEffect, useMemo, useRef, useState } from 'react';

const fixtureModules = import.meta.glob('../../../fixtures/*.walu', {
  eager: true,
  query: '?raw',
  import: 'default'
});

const PRESETS = Object.entries(fixtureModules)
  .map(([path, source]) => ({
    key: path.split('/').pop().replace(/\.walu$/, ''),
    label: path
      .split('/').pop()
      .replace(/\.walu$/, '')
      .split('-')
      .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
      .join(' '),
    source
  }))
  .sort((left, right) => left.label.localeCompare(right.label));

const DEFAULT_PRESET = PRESETS[0]?.source ?? '';

// Parse WebAssembly binary to extract exports and signatures
function getWasmExports(buffer) {
  if (!buffer) return [];
  const bytes = new Uint8Array(buffer);
  let pos = 8; // Skip magic number and version
  
  // Helper to read LEB128 unsigned integer
  function readVaruint() {
    let result = 0;
    let shift = 0;
    while (true) {
      if (pos >= bytes.length) return result;
      const byte = bytes[pos++];
      result |= (byte & 0x7f) << shift;
      if ((byte & 0x80) === 0) break;
      shift += 7;
    }
    return result;
  }

  const types = [];
  const funcTypeIndices = [];
  const exports = [];

  while (pos < bytes.length) {
    const sectionId = bytes[pos++];
    const sectionLength = readVaruint();
    const sectionEnd = pos + sectionLength;
    if (sectionEnd > bytes.length) break;

    if (sectionId === 1) { // Type section
      const numTypes = readVaruint();
      for (let i = 0; i < numTypes; i++) {
        const form = bytes[pos++]; // 0x60 for function
        if (form === 0x60) {
          const numParams = readVaruint();
          const params = [];
          for (let p = 0; p < numParams; p++) {
            params.push(bytes[pos++]);
          }
          const numReturns = readVaruint();
          const returns = [];
          for (let r = 0; r < numReturns; r++) {
            returns.push(bytes[pos++]);
          }
          types.push({ params, returns });
        }
      }
    } else if (sectionId === 3) { // Function section
      const numFuncs = readVaruint();
      for (let i = 0; i < numFuncs; i++) {
        funcTypeIndices.push(readVaruint());
      }
    } else if (sectionId === 7) { // Export section
      const numExports = readVaruint();
      for (let i = 0; i < numExports; i++) {
        const nameLen = readVaruint();
        const nameBytes = bytes.subarray(pos, pos + nameLen);
        pos += nameLen;
        const name = new TextDecoder().decode(nameBytes);
        const kind = bytes[pos++];
        const index = readVaruint();
        if (kind === 0) { // function export
          exports.push({ name, index });
        }
      }
    } else {
      pos = sectionEnd;
    }
  }

  return exports.map(exp => {
    const typeIdx = funcTypeIndices[exp.index];
    const signature = types[typeIdx] || { params: [], returns: [] };
    return {
      name: exp.name,
      params: signature.params.map(typeCode => {
        if (typeCode === 0x7f) return 'i32';
        if (typeCode === 0x7e) return 'i64';
        if (typeCode === 0x7d) return 'f32';
        if (typeCode === 0x7c) return 'f64';
        return 'unknown';
      }),
      returns: signature.returns.map(typeCode => {
        if (typeCode === 0x7f) return 'i32';
        if (typeCode === 0x7e) return 'i64';
        if (typeCode === 0x7d) return 'f32';
        if (typeCode === 0x7c) return 'f64';
        return 'unknown';
      })
    };
  });
}

function executeCall(instance, funcName, paramsInfo, inputValues) {
  if (!instance) return { error: 'No instance' };
  const func = instance.exports[funcName];
  if (!func) return { error: `Exported function "${funcName}" not found` };

  try {
    const parsedArgs = [];
    for (let i = 0; i < paramsInfo.length; i++) {
      const type = paramsInfo[i];
      const valStr = inputValues[i] || '0';
      
      if (type === 'i64') {
        try {
          parsedArgs.push(BigInt(valStr.trim().replace(/n$/, '')));
        } catch {
          return { error: `Parameter ${i} must be a valid 64-bit integer` };
        }
      } else if (type === 'i32') {
        const val = Number(valStr);
        if (isNaN(val) || !Number.isInteger(val)) {
          return { error: `Parameter ${i} must be a valid 32-bit integer` };
        }
        parsedArgs.push(val);
      } else if (type === 'f32' || type === 'f64') {
        const val = Number(valStr);
        if (isNaN(val)) {
          return { error: `Parameter ${i} must be a valid number` };
        }
        parsedArgs.push(val);
      } else {
        parsedArgs.push(Number(valStr));
      }
    }

    const result = func(...parsedArgs);
    if (typeof result === 'bigint') {
      return { value: result.toString() + 'n' };
    } else {
      return { value: String(result) };
    }
  } catch (err) {
    return { error: `Execution crashed: ${err.message}` };
  }
}

function classifyWasmInstantiationError(err, requiresWasmGc) {
  const message = err?.message || String(err);
  if (!requiresWasmGc) {
    return `Failed to instantiate WASM module: ${message}`;
  }
  return [
    'This module requires Wasm GC (array reference types), but this browser runtime does not support it yet.',
    `Runtime error: ${message}`
  ].join('\n');
}

export default function App() {
  const [code, setCode] = useState(DEFAULT_PRESET);
  const [status, setStatus] = useState('loading'); // 'loading', 'ready', 'success', 'error'
  const [loadErrorMsg, setLoadErrorMsg] = useState('');
  const [compilerReady, setCompilerReady] = useState(false);
  const [compileSource, setCompileSource] = useState(null);
  const [activeTab, setActiveTab] = useState('ir'); // 'ir', 'logs', 'run'
  const [runInstance, setRunInstance] = useState(null);
  const [runError, setRunError] = useState(null);
  const [exportsList, setExportsList] = useState([]);
  const [funcInputs, setFuncInputs] = useState({});
  const [autoRun, setAutoRun] = useState(true);
  const [manualResults, setManualResults] = useState({});
  
  const textareaRef = useRef(null);
  const lineNumbersRef = useRef(null);

  // Sync scroll of line numbers and textarea
  const handleScroll = () => {
    if (textareaRef.current && lineNumbersRef.current) {
      lineNumbersRef.current.scrollTop = textareaRef.current.scrollTop;
    }
  };

  // Load wasm-bindgen compiler module on mount.
  useEffect(() => {
    let cancelled = false;

    import('./waluau-wasm/waluau_wasm.js')
      .then(async (module) => {
        await module.default();
        if (cancelled) {
          return;
        }
        setCompileSource(() => module.compile);
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

  const compilation = useMemo(() => {
    if (!compilerReady || !compileSource) {
      return {
        output: '',
        errorMsg: status === 'error' ? loadErrorMsg : '',
      };
    }

    try {
      const parsed = compileSource(code);
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
  }, [code, compileSource, compilerReady, loadErrorMsg, status]);

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

  // Split lines for line numbering
  const lineCount = code.split('\n').length;
  const lineNumbers = Array.from({ length: Math.max(lineCount, 1) }, (_, i) => i + 1);

  const selectPreset = (source) => {
    setCode(source);
  };

  // Sync runInstance, exportsList, inputs and results when wasm changes
  useEffect(() => {
    let active = true;
    async function loadModule() {
      await Promise.resolve(); // Yield to prevent synchronous setState warnings
      if (!outputWasmBytes) {
        if (active) {
          setRunInstance(null);
          setRunError(null);
          setExportsList([]);
          setManualResults({});
        }
        return;
      }
      try {
        const wasmBuffer = new Uint8Array(outputWasmBytes);
        const list = getWasmExports(wasmBuffer);
        const obj = await WebAssembly.instantiate(wasmBuffer, {});
        
        if (active) {
          setRunInstance(obj.instance);
          setExportsList(list);
          setRunError(null);
          setManualResults({});
          
          setFuncInputs(prev => {
            const next = { ...prev };
            let changed = false;
            for (const func of list) {
              if (!next[func.name] || next[func.name].length !== func.params.length) {
                next[func.name] = func.params.map(() => '0');
                changed = true;
              }
            }
            return changed ? next : prev;
          });
        }
      } catch (err) {
        if (active) {
          console.error("Instantiation failed:", err);
          setRunInstance(null);
          setExportsList([]);
          setRunError(classifyWasmInstantiationError(err, requiresWasmGc));
          setManualResults({});
        }
      }
    }

    loadModule();

    return () => {
      active = false;
    };
  }, [outputWasmBytes, requiresWasmGc]);

  const handleInputChange = (funcName, paramIndex, value) => {
    setFuncInputs(prev => {
      const funcParams = prev[funcName] ? [...prev[funcName]] : [];
      funcParams[paramIndex] = value;
      return { ...prev, [funcName]: funcParams };
    });
  };

  const handleManualRun = (funcName, params) => {
    const inputs = funcInputs[funcName] || params.map(() => '0');
    const res = executeCall(runInstance, funcName, params, inputs);
    setManualResults(prev => ({ ...prev, [funcName]: res }));
  };

  const getResult = (funcName, params) => {
    if (autoRun) {
      const inputs = funcInputs[funcName] || params.map(() => '0');
      return executeCall(runInstance, funcName, params, inputs);
    } else {
      return manualResults[funcName] || { isIdle: true };
    }
  };

  return (
    <div className="app-container">
      {/* Background radial glow */}
      <div className="glow-effect glow-top-left"></div>
      <div className="glow-effect glow-bottom-right"></div>

      <header className="app-header">
        <div className="header-brand">
          <div className="logo-icon">W</div>
          <div>
            <h1>Waluau</h1>
            <p className="subtitle">Compiler Playground & Wasm Codegen</p>
          </div>
        </div>

        <div className="header-controls">
          {/* Status Badge */}
          <div className={`status-badge ${displayStatus}`}>
            <span className="pulse-dot"></span>
            <span className="status-text">
              {displayStatus === 'loading' && 'Loading Compiler...'}
              {displayStatus === 'ready' && 'Ready'}
              {displayStatus === 'success' && 'Compilation Succeeded'}
              {displayStatus === 'error' && 'Compilation Failed'}
            </span>
          </div>
        </div>
      </header>

      {/* Preset toolbar */}
      <div className="presets-bar">
        <span className="presets-label">Examples:</span>
        {PRESETS.map((preset) => (
          <button key={preset.key} className="preset-btn" onClick={() => selectPreset(preset.source)}>
            {preset.label}
          </button>
        ))}
      </div>

      <main className="playground-main">
        {/* Editor Column */}
        <section className="column-panel editor-panel">
          <div className="panel-header">
            <h3>Source Code</h3>
            <span className="file-extension">.walu</span>
          </div>
          <div className="editor-container">
            <div className="line-numbers" ref={lineNumbersRef}>
              {lineNumbers.map((num) => (
                <div key={num} className="line-num">
                  {num}
                </div>
              ))}
            </div>
            <textarea
              ref={textareaRef}
              className="code-textarea"
              value={code}
              onChange={(e) => setCode(e.target.value)}
              onScroll={handleScroll}
              spellCheck="false"
              placeholder="-- Enter your compiler code here..."
            />
          </div>
        </section>

        {/* Output Column */}
        <section className="column-panel output-panel">
          <div className="panel-header tab-header">
            <div className="tab-buttons">
              <button
                className={`tab-btn ${activeTab === 'ir' ? 'active' : ''}`}
                onClick={() => setActiveTab('ir')}
              >
                Generated IR
              </button>
              <button
                className={`tab-btn ${activeTab === 'wat' ? 'active' : ''}`}
                onClick={() => setActiveTab('wat')}
              >
                Wasm Text (WAT)
              </button>
              <button
                className={`tab-btn ${activeTab === 'logs' ? 'active' : ''}`}
                onClick={() => setActiveTab('logs')}
              >
                Compiler Diagnostics
              </button>
              <button
                className={`tab-btn ${activeTab === 'run' ? 'active' : ''}`}
                onClick={() => setActiveTab('run')}
              >
                Function Calling
              </button>
            </div>
          </div>

          <div className="tab-content">
            {activeTab === 'ir' && (
              <div className="output-container">
                {status === 'loading' ? (
                  <div className="loading-state">
                    <div className="spinner"></div>
                    <p>Initializing Waluau compiler module...</p>
                  </div>
                ) : displayStatus === 'error' && !outputIr ? (
                  <div className="error-state">
                    <svg className="error-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                      <circle cx="12" cy="12" r="10" />
                      <line x1="12" y1="8" x2="12" y2="12" />
                      <line x1="12" y1="16" x2="12.01" y2="16" />
                    </svg>
                    <h4>Compilation Error</h4>
                    <pre className="diagnostic-output">{errorMsg}</pre>
                  </div>
                ) : (
                  <pre className="ir-output">
                    <code>{outputIr || '-- No IR output generated. Write some valid code first.'}</code>
                  </pre>
                )}
              </div>
            )}

            {activeTab === 'wat' && (
              <div className="output-container">
                {status === 'loading' ? (
                  <div className="loading-state">
                    <div className="spinner"></div>
                    <p>Initializing Waluau compiler module...</p>
                  </div>
                ) : displayStatus === 'error' && !outputWat ? (
                  <div className="error-state">
                    <svg className="error-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                      <circle cx="12" cy="12" r="10" />
                      <line x1="12" y1="8" x2="12" y2="12" />
                      <line x1="12" y1="16" x2="12.01" y2="16" />
                    </svg>
                    <h4>Compilation Error</h4>
                    <pre className="diagnostic-output">{errorMsg}</pre>
                  </div>
                ) : (
                  <pre className="wat-output">
                    <code>{outputWat || '-- No WAT output generated. Write some valid code first.'}</code>
                  </pre>
                )}
              </div>
            )}

            {activeTab === 'logs' && (
              <div className="logs-container">
                <h3>Compilation Log</h3>
                 <div className={`log-card ${displayStatus}`}>
                  <div className="log-header">
                    <span className="log-bullet"></span>
                    <span className="log-title">
                      {displayStatus === 'success' && 'Build Succeeded'}
                      {displayStatus === 'error' && 'Build Failed'}
                      {displayStatus === 'ready' && 'Ready'}
                      {displayStatus === 'loading' && 'Loading...'}
                    </span>
                  </div>
                  <div className="log-body">
                    {displayStatus === 'success' && (
                      <p className="success-text">
                        The program parsed and type-checked successfully! The intermediate representation was constructed and verified without errors.
                      </p>
                    )}
                    {displayStatus === 'error' && (
                      <div className="error-details">
                        <p className="failure-text">Compiler diagnostics returned the following error:</p>
                        <pre className="error-block">{errorMsg}</pre>
                      </div>
                    )}
                    {displayStatus === 'ready' && <p>No compilation has run yet. Enter code to trigger compiler.</p>}
                  </div>
                </div>
              </div>
            )}

            {activeTab === 'run' && (
              <div className="func-calling-container">
                <div className="func-calling-header">
                  <h3>Function Calling</h3>
                  <label className="autorun-toggle">
                    <input
                      type="checkbox"
                      checked={autoRun}
                      onChange={(e) => setAutoRun(e.target.checked)}
                    />
                    Auto-run on input change
                  </label>
                </div>

                {status === 'loading' ? (
                  <div className="loading-state">
                    <div className="spinner"></div>
                    <p>Initializing compiler...</p>
                  </div>
                ) : runError ? (
                  <div className="error-state">
                    <svg className="error-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                      <circle cx="12" cy="12" r="10" />
                      <line x1="12" y1="8" x2="12" y2="12" />
                      <line x1="12" y1="16" x2="12.01" y2="16" />
                    </svg>
                    <h4>Instantiation Error</h4>
                    <pre className="diagnostic-output">{runError}</pre>
                  </div>
                ) : exportsList.length === 0 ? (
                  <div className="loading-state">
                    <p>No functions exported from this WASM module.</p>
                  </div>
                ) : (
                  <div className="func-list">
                    {exportsList.map((func) => {
                      const inputs = funcInputs[func.name] || func.params.map(() => '0');
                      const res = getResult(func.name, func.params);

                      return (
                        <div key={func.name} className="func-card">
                          <div className="func-signature">
                            <span className="func-signature-name">{func.name}</span>
                            <span>(</span>
                            {func.params.map((type, idx) => (
                              <span key={idx}>
                                param{idx}: <span className="func-signature-type">{type}</span>
                                {idx < func.params.length - 1 ? ', ' : ''}
                              </span>
                            ))}
                            <span>)</span>
                            <span className="func-signature-arrow"> -&gt; </span>
                            <span className="func-signature-type">
                              {func.returns.length > 0 ? func.returns.join(', ') : 'void'}
                            </span>
                          </div>

                          {func.params.length > 0 && (
                            <div className="func-inputs">
                              {func.params.map((type, idx) => (
                                <div key={idx} className="func-input-row">
                                  <label className="func-input-label">param{idx} ({type}):</label>
                                  <input
                                    type="text"
                                    className="func-input-field"
                                    value={inputs[idx] ?? '0'}
                                    onChange={(e) => handleInputChange(func.name, idx, e.target.value)}
                                    placeholder={`Enter ${type} value`}
                                  />
                                </div>
                              ))}
                            </div>
                          )}

                          {!autoRun && (
                            <button
                              className="func-run-btn"
                              onClick={() => handleManualRun(func.name, func.params)}
                            >
                              Run Function
                            </button>
                          )}

                          <div className={`func-result-box ${res.error ? 'error' : res.isIdle ? '' : 'success'}`}>
                            <div className="func-result-label">Result</div>
                            {res.isIdle ? (
                              <div className="func-result-value idle">Click "Run Function" to execute</div>
                            ) : res.error ? (
                              <div className="func-result-value error">{res.error}</div>
                            ) : (
                              <div className="func-result-value success">{res.value}</div>
                            )}
                          </div>
                        </div>
                      );
                    })}
                  </div>
                )}
              </div>
            )}
          </div>
        </section>
      </main>
    </div>
  );
}
