import { useCallback, useState, useEffect, useRef } from 'react';
import { getDefaultParamValue, renderType } from '../utils/wasm.js';
import domOutputCss from '../dom-output.css?inline';
import { ParamField } from './ParamFields.jsx';

const DOM_OUTPUT_SRC_DOC = `<!doctype html>
<html>
  <head>
    <meta name="tailwind-compatible-polyfill" content="local deterministic utility subset">
    <meta http-equiv="Content-Security-Policy" content="script-src 'none'; object-src 'none'; base-uri 'none'">
    <style>${domOutputCss}</style>
  </head>
  <body></body>
</html>`;

function DomOutputFrame({
  setDomOutputRoot,
  onEscape,
  isFullscreen,
  exportsList,
  status,
  runError,
}) {
  const onEscapeRef = useRef(onEscape);
  useEffect(() => {
    onEscapeRef.current = onEscape;
  }, [onEscape]);

  const iframeRef = useRef(null);

  const setFrame = useCallback((node) => {
    iframeRef.current = node;
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

  useEffect(() => {
    if (isFullscreen && iframeRef.current) {
      if (status === 'success' || (status === 'ready' && !runError)) {
        requestAnimationFrame(() => {
          iframeRef.current?.contentWindow?.focus();
        });
      }
    }
  }, [isFullscreen, exportsList, status, runError]);

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
          <DomOutputFrame
            setDomOutputRoot={setDomOutputRoot}
            onEscape={() => setIsFullscreen(false)}
            isFullscreen={isFullscreen}
            exportsList={exportsList}
            status={status}
            runError={runError}
          />
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
