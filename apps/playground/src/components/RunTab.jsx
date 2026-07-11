import { useCallback, useState, useEffect, useRef } from 'react';
import { getDefaultParamValue, renderType } from '../utils/wasm.js';
import { ParamField } from './ParamFields.jsx';

const DOM_OUTPUT_SRC_DOC = `<!doctype html>
<html>
  <head>
    <meta name="tailwind-compatible-polyfill" content="local deterministic utility subset">
    <meta http-equiv="Content-Security-Policy" content="script-src 'none'; object-src 'none'; base-uri 'none'">
    <style>
      *{box-sizing:border-box}
      html{color:#1f2937;font-family:system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;font-size:16px;line-height:1.5}
      body{margin:0;padding:0;background:#f8fafc}
      h1,h2,h3,p{margin:0}
      button,input,textarea{font:inherit}
      button{cursor:pointer}
      .flex{display:flex}.grid{display:grid}.hidden{display:none}
      .flex-col{flex-direction:column}.flex-wrap{flex-wrap:wrap}.items-start{align-items:flex-start}.items-center{align-items:center}.justify-between{justify-content:space-between}.justify-end{justify-content:flex-end}
      .flex-1{flex:1 1 0%}.min-w-0{min-width:0}.min-h-0{min-height:0}.h-screen{height:100vh}.w-full{width:100%}.min-h-24{min-height:6rem}
      .grid-cols-1{grid-template-columns:repeat(1,minmax(0,1fr))}
      .gap-1{gap:.25rem}.gap-2{gap:.5rem}.gap-3{gap:.75rem}.gap-4{gap:1rem}
      .space-y-1>:not([hidden])~:not([hidden]){margin-top:.25rem}
      .overflow-hidden{overflow:hidden}.overflow-y-auto{overflow-y:auto}.truncate{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
      .rounded{border-radius:.25rem}.rounded-md{border-radius:.375rem}
      .border{border-width:1px;border-style:solid}.border-b{border-bottom-width:1px;border-bottom-style:solid}.border-t{border-top-width:1px;border-top-style:solid}
      .border-slate-200{border-color:#e2e8f0}.border-slate-300{border-color:#cbd5e1}.border-teal-300{border-color:#5eead4}.border-rose-200{border-color:#fecdd3}
      .bg-white{background-color:#fff}.bg-slate-50{background-color:#f8fafc}.bg-slate-100{background-color:#f1f5f9}.bg-teal-700{background-color:#0f766e}
      .text-white{color:#fff}.text-slate-950{color:#020617}.text-slate-900{color:#0f172a}.text-slate-700{color:#334155}.text-slate-600{color:#475569}.text-slate-500{color:#64748b}.text-rose-700{color:#be123c}
      .p-3{padding:.75rem}.p-4{padding:1rem}.px-2{padding-left:.5rem;padding-right:.5rem}.px-3{padding-left:.75rem;padding-right:.75rem}.px-4{padding-left:1rem;padding-right:1rem}.px-6{padding-left:1.5rem;padding-right:1.5rem}
      .py-1{padding-top:.25rem;padding-bottom:.25rem}.py-2{padding-top:.5rem;padding-bottom:.5rem}.py-3{padding-top:.75rem;padding-bottom:.75rem}.py-4{padding-top:1rem;padding-bottom:1rem}
      .mt-1{margin-top:.25rem}.mt-2{margin-top:.5rem}.mt-3{margin-top:.75rem}
      .text-xs{font-size:.75rem;line-height:1rem}.text-sm{font-size:.875rem;line-height:1.25rem}.text-xl{font-size:1.25rem;line-height:1.75rem}
      .font-medium{font-weight:500}.font-semibold{font-weight:600}.uppercase{text-transform:uppercase}.leading-5{line-height:1.25rem}
      .outline-none{outline:2px solid transparent;outline-offset:2px}.shadow-sm{box-shadow:0 1px 2px 0 rgb(0 0 0 / .05)}
      .hover\\:bg-teal-800:hover{background-color:#115e59}.hover\\:bg-slate-50:hover{background-color:#f8fafc}.hover\\:bg-rose-50:hover{background-color:#fff1f2}
      .focus\\:border-teal-600:focus{border-color:#0d9488}
      @media (min-width:768px){
        .md\\:grid-cols-3{grid-template-columns:repeat(3,minmax(0,1fr))}
        .md\\:flex-row{flex-direction:row}
        .md\\:items-center{align-items:center}
        .md\\:justify-between{justify-content:space-between}
      }
    </style>
  </head>
  <body></body>
</html>`;

function DomOutputFrame({ setDomOutputRoot, onEscape }) {
  const onEscapeRef = useRef(onEscape);
  useEffect(() => {
    onEscapeRef.current = onEscape;
  }, [onEscape]);

  const setFrame = useCallback((node) => {
    if (!node) {
      setDomOutputRoot(null);
      return;
    }
    let doc = null;
    const handleKeyDown = (e) => {
      if (e.key === 'Escape' && onEscapeRef.current) {
        onEscapeRef.current();
      }
    };
    const syncDocument = () => {
      const nextDoc = node.contentDocument;
      if (!nextDoc?.querySelector('meta[name="tailwind-compatible-polyfill"]')) {
        setDomOutputRoot(null);
        return;
      }
      if (doc) {
        doc.removeEventListener('keydown', handleKeyDown);
      }
      doc = nextDoc;
      doc.addEventListener('keydown', handleKeyDown);
      setDomOutputRoot(doc);
    };
    syncDocument();
    node.addEventListener('load', syncDocument);
    return () => {
      if (doc) {
        doc.removeEventListener('keydown', handleKeyDown);
      }
      node.removeEventListener('load', syncDocument);
      setDomOutputRoot(null);
    };
  }, [setDomOutputRoot]);

  return (
    <iframe
      className="dom-output-frame"
      ref={setFrame}
      title="DOM Output"
      sandbox="allow-same-origin allow-scripts"
      srcDoc={DOM_OUTPUT_SRC_DOC}
    />
  );
}

export default function RunTab({
  status,
  runError,
  exportsList,
  funcInputs,
  initLogs,
  usesDomOutput,
  setDomOutputRoot,
  autoRun,
  setAutoRun,
  handleInputChange,
  handleRecordFieldChange,
  handleManualRun,
  getResult,
}) {
  const [isFullscreen, setIsFullscreen] = useState(() => {
    const params = new URLSearchParams(window.location.search);
    return params.get('fullscreen') === 'true';
  });

  useEffect(() => {
    const url = new URL(window.location.href);
    if (isFullscreen) {
      url.searchParams.set('fullscreen', 'true');
    } else {
      url.searchParams.delete('fullscreen');
    }
    window.history.replaceState(null, '', url);
  }, [isFullscreen]);


  useEffect(() => {
    if (!isFullscreen) return;
    const handleKeyDown = (e) => {
      if (e.key === 'Escape') {
        setIsFullscreen(false);
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => {
      window.removeEventListener('keydown', handleKeyDown);
    };
  }, [isFullscreen]);

  return (
    <div className="func-calling-container">
      <div className="func-calling-header">
        <h3>Run</h3>
        <label className="autorun-toggle">
          <input
            type="checkbox"
            checked={autoRun}
            onChange={(e) => setAutoRun(e.target.checked)}
          />
          Auto-run on input change
        </label>
      </div>

      {initLogs.length > 0 && (
        <div className="init-logs-box">
          <div className="init-logs-label">Module Initialization Print Output</div>
          <pre className="init-logs-value">{initLogs.join('\n')}</pre>
        </div>
      )}

      {usesDomOutput && (
        <section className={`dom-output-section ${isFullscreen ? 'fullscreen' : ''}`} aria-label="DOM Output">
          <div className="dom-output-header">
            <div className="dom-output-label">DOM Output</div>
            <button
              type="button"
              className="dom-output-fullscreen-btn"
              onClick={() => setIsFullscreen(true)}
              title="Full Screen"
            >
              <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                <path d="M8 3H5a2 2 0 0 0-2 2v3m18 0V5a2 2 0 0 0-2-2h-3m0 18h3a2 2 0 0 0 2-2v-3M3 16v3a2 2 0 0 0 2 2h3" />
              </svg>
              <span>Full Screen</span>
            </button>
          </div>
          {isFullscreen && (
            <div className="dom-output-fullscreen-bar">
              <span className="dom-output-fullscreen-title">DOM Output (Full Screen)</span>
              <button
                type="button"
                className="dom-output-exit-btn"
                onClick={() => setIsFullscreen(false)}
                title="Exit Full Screen"
              >
                <span>Close Full Screen</span>
                <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                  <line x1="18" y1="6" x2="6" y2="18" />
                  <line x1="6" y1="6" x2="18" y2="18" />
                </svg>
              </button>
            </div>
          )}
          <DomOutputFrame setDomOutputRoot={setDomOutputRoot} onEscape={() => setIsFullscreen(false)} />
        </section>
      )}

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
          <h4>Module Load Error</h4>
          <pre className="diagnostic-output">{runError}</pre>
        </div>
      ) : exportsList.length === 0 ? (
        <div className="loading-state">
          <p>No functions exported from this WASM module.</p>
        </div>
      ) : (
        <div className="func-list">
          {exportsList.map((func) => {
            const inputs = funcInputs[func.name] || (func.richParams || func.params).map(getDefaultParamValue);
            const res = getResult(func.name, func.params, func.richParams, func.richReturns);

            return (
              <div key={func.name} className="func-card">
                <div className="func-signature">
                  <span className="func-signature-name">{func.name}</span>
                  <span>(</span>
                  {func.richParams ? (
                    func.richParams.map((type, idx) => (
                      <span key={idx}>
                        param{idx}: <span className="func-signature-type">{renderType(type)}</span>
                        {idx < func.richParams.length - 1 ? ', ' : ''}
                      </span>
                    ))
                  ) : (
                    func.params.map((type, idx) => (
                      <span key={idx}>
                        param{idx}: <span className="func-signature-type">{type}</span>
                        {idx < func.params.length - 1 ? ', ' : ''}
                      </span>
                    ))
                  )}
                  <span>)</span>
                  <span className="func-signature-arrow"> -&gt; </span>
                  <span className="func-signature-type">
                    {func.richReturns ? (
                      func.richReturns.length > 0 ? func.richReturns.map(renderType).join(', ') : 'void'
                    ) : (
                      func.returns.length > 0 ? func.returns.join(', ') : 'void'
                    )}
                  </span>
                </div>

                {func.params.length > 0 && (
                  <div className="func-inputs">
                    {func.params.map((type, idx) => {
                      const richType = func.richParams ? func.richParams[idx] : null;
                      return (
                        <ParamField
                          key={idx}
                          funcName={func.name}
                          paramIdx={idx}
                          type={type}
                          richType={richType}
                          val={inputs[idx]}
                          handleInputChange={handleInputChange}
                          handleRecordFieldChange={handleRecordFieldChange}
                          isInline={false}
                        />
                      );
                    })}
                  </div>
                )}

                {!autoRun && (
                  <button
                    className="func-run-btn"
                    onClick={() => handleManualRun(func.name, func.params, func.richParams, func.richReturns)}
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

                {res.logs && res.logs.length > 0 && (
                  <div className="func-logs-box">
                    <div className="func-logs-label">Print Output</div>
                    <pre className="func-logs-value">{res.logs.join('\n')}</pre>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
