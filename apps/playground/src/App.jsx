import React, { useState, useEffect, useRef } from 'react';

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

export default function App() {
  const [code, setCode] = useState(DEFAULT_PRESET);
  const [output, setOutput] = useState('');
  const [status, setStatus] = useState('loading'); // 'loading', 'ready', 'success', 'error'
  const [errorMsg, setErrorMsg] = useState('');
  const [activeTab, setActiveTab] = useState('ir'); // 'ir', 'logs', 'info'
  const [wasmInstance, setWasmInstance] = useState(null);
  
  const textareaRef = useRef(null);
  const lineNumbersRef = useRef(null);

  // Sync scroll of line numbers and textarea
  const handleScroll = () => {
    if (textareaRef.current && lineNumbersRef.current) {
      lineNumbersRef.current.scrollTop = textareaRef.current.scrollTop;
    }
  };

  // Load WASM on mount
  useEffect(() => {
    async function initWasm() {
      try {
        const response = await fetch('/waluau_wasm.wasm');
        if (!response.ok) {
          throw new Error(`Failed to fetch WASM module: ${response.statusText}`);
        }
        const buffer = await response.arrayBuffer();
        const obj = await WebAssembly.instantiate(buffer, { env: {} });
        setWasmInstance(obj.instance);
        setStatus('ready');
      } catch (err) {
        console.error('WASM load error:', err);
        setStatus('error');
        setErrorMsg(`Failed to load WASM compiler: ${err.message}`);
      }
    }
    initWasm();
  }, []);

  // Run compiler when code or WASM instance changes
  useEffect(() => {
    if (!wasmInstance) return;

    try {
      // 1. Encode source string to UTF-8 bytes
      const encoder = new TextEncoder();
      const sourceBytes = encoder.encode(code);
      const len = sourceBytes.length;

      // 2. Allocate memory in WASM for the source code
      const ptr = wasmInstance.exports.alloc(len);

      // 3. Write source bytes into the WASM memory buffer
      const memory = new Uint8Array(wasmInstance.exports.memory.buffer);
      memory.set(sourceBytes, ptr);

      // 4. Call compile function
      const resultPtr = wasmInstance.exports.compile(ptr, len);

      // 5. Read the null-terminated result string from WASM memory
      const resultMemory = new Uint8Array(wasmInstance.exports.memory.buffer);
      let end = resultPtr;
      while (resultMemory[end] !== 0) {
        end++;
      }
      const decoder = new TextDecoder();
      const rawResult = decoder.decode(resultMemory.subarray(resultPtr, end));

      // 6. Free allocated memory in WASM
      wasmInstance.exports.free_string(resultPtr);
      wasmInstance.exports.dealloc(ptr, len);

      // Parse output
      if (rawResult.startsWith('Success:\n')) {
        setOutput(rawResult.substring(9));
        setStatus('success');
        setErrorMsg('');
      } else if (rawResult.startsWith('Error:\n')) {
        setOutput('');
        setStatus('error');
        setErrorMsg(rawResult.substring(7));
      } else {
        setOutput(rawResult);
        setStatus('ready');
        setErrorMsg('');
      }
    } catch (err) {
      console.error('Compilation error:', err);
      setStatus('error');
      setErrorMsg(`Compilation crashed: ${err.message}`);
    }
  }, [code, wasmInstance]);

  // Split lines for line numbering
  const lineCount = code.split('\n').length;
  const lineNumbers = Array.from({ length: Math.max(lineCount, 1) }, (_, i) => i + 1);

  const selectPreset = (source) => {
    setCode(source);
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
          <div className={`status-badge ${status}`}>
            <span className="pulse-dot"></span>
            <span className="status-text">
              {status === 'loading' && 'Loading Compiler...'}
              {status === 'ready' && 'Ready'}
              {status === 'success' && 'Compilation Succeeded'}
              {status === 'error' && 'Compilation Failed'}
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
                className={`tab-btn ${activeTab === 'logs' ? 'active' : ''}`}
                onClick={() => setActiveTab('logs')}
              >
                Compiler Diagnostics
              </button>
              <button
                className={`tab-btn ${activeTab === 'info' ? 'active' : ''}`}
                onClick={() => setActiveTab('info')}
              >
                Language Guide
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
                ) : status === 'error' && !output ? (
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
                    <code>{output || '-- No IR output generated. Write some valid code first.'}</code>
                  </pre>
                )}
              </div>
            )}

            {activeTab === 'logs' && (
              <div className="logs-container">
                <h3>Compilation Log</h3>
                <div className={`log-card ${status}`}>
                  <div className="log-header">
                    <span className="log-bullet"></span>
                    <span className="log-title">
                      {status === 'success' && 'Build Succeeded'}
                      {status === 'error' && 'Build Failed'}
                      {status === 'ready' && 'Ready'}
                      {status === 'loading' && 'Loading...'}
                    </span>
                  </div>
                  <div className="log-body">
                    {status === 'success' && (
                      <p className="success-text">
                        The program parsed and type-checked successfully! The intermediate representation was constructed and verified without errors.
                      </p>
                    )}
                    {status === 'error' && (
                      <div className="error-details">
                        <p className="failure-text">Compiler diagnostics returned the following error:</p>
                        <pre className="error-block">{errorMsg}</pre>
                      </div>
                    )}
                    {status === 'ready' && <p>No compilation has run yet. Enter code to trigger compiler.</p>}
                  </div>
                </div>
              </div>
            )}

            {activeTab === 'info' && (
              <div className="info-container">
                <h3>Waluau Language syntax guide</h3>
                <p>
                  Waluau is a statically-typed scripting language with syntax inspired by Lua. It compiles down to a structured intermediate representation (IR) ready for Wasm code generation.
                </p>

                <h4>Types</h4>
                <ul>
                  <li><code>number</code>: Double-precision floating point numbers.</li>
                  <li><code>bool</code>: Boolean values (<code>true</code> / <code>false</code>).</li>
                </ul>

                <h4>Function Declaration</h4>
                <pre className="syntax-code">
{`fn functionName(param1: type, param2: type) -> returnType
    -- body
    return val
end`}
                </pre>

                <h4>Variables & Assignment</h4>
                <pre className="syntax-code">
{`let name: type = val -- Variable declaration
name = newVal        -- Variable assignment`}
                </pre>

                <h4>Control Flow</h4>
                <pre className="syntax-code">
{`if condition then
    -- then body
else
    -- else body
end

while condition do
    -- loop body
end`}
                </pre>
              </div>
            )}
          </div>
        </section>
      </main>
    </div>
  );
}
