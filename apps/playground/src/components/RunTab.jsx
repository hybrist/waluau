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
  onClose,
  isFullscreen,
  exportsList,
  status,
  runError,
}) {
  const onCloseRef = useRef(onClose);
  useEffect(() => {
    onCloseRef.current = onClose;
  }, [onClose]);

  const iframeRef = useRef(null);
  const popupRef = useRef(null);
  const [popupBlocked, setPopupBlocked] = useState(false);

  useEffect(() => {
    if (!isFullscreen) {
      return;
    }

    let popup = null;
    try {
      popup = window.open('', 'waluau-preview');
    } catch (e) {
      console.warn('Failed to open preview window:', e);
    }

    if (!popup) {
      queueMicrotask(() => setPopupBlocked(true));
      return;
    }

    queueMicrotask(() => setPopupBlocked(false));
    popupRef.current = popup;

    popup.document.open();
    popup.document.write(DOM_OUTPUT_SRC_DOC);
    popup.document.close();
    popup.focus();

    setDomOutputRoot(popup.document);

    const handleClose = () => {
      if (onCloseRef.current) {
        onCloseRef.current();
      }
    };

    popup.addEventListener('pagehide', handleClose);

    const interval = setInterval(() => {
      if (popup.closed) {
        handleClose();
      }
    }, 300);

    return () => {
      clearInterval(interval);
      if (popup) {
        popup.removeEventListener('pagehide', handleClose);
        if (!popup.closed) {
          popup.close();
        }
      }
      popupRef.current = null;
    };
  }, [isFullscreen, setDomOutputRoot]);

  useEffect(() => {
    if (isFullscreen && popupRef.current) {
      if (status === 'success' || (status === 'ready' && !runError)) {
        requestAnimationFrame(() => {
          if (popupRef.current && !popupRef.current.closed) {
            // Do not steal focus if the user is actively focusing an editor or input on the main page
            const active = document.activeElement;
            const isTyping = active && (
              active.tagName === 'INPUT' ||
              active.tagName === 'TEXTAREA' ||
              (typeof active.closest === 'function' && (
                active.closest('.monaco-editor') ||
                active.closest('.repl-input')
              ))
            );
            if (!isTyping) {
              popupRef.current.focus();
            }
          }
        });
      }
    }
  }, [isFullscreen, exportsList, status, runError]);

  const setFrame = useCallback((node) => {
    iframeRef.current = node;
    if (!node) {
      if (!isFullscreen) {
        setDomOutputRoot(null);
      }
      return;
    }
    let doc = null;
    const syncDocument = () => {
      const nextDoc = node.contentDocument;
      if (!nextDoc?.querySelector('meta[name="tailwind-compatible-polyfill"]')) {
        if (!isFullscreen) {
          setDomOutputRoot(null);
        }
        return;
      }
      doc = nextDoc;
      if (!isFullscreen) {
        setDomOutputRoot(doc);
      }
    };
    syncDocument();
    node.addEventListener('load', syncDocument);
    return () => {
      node.removeEventListener('load', syncDocument);
      if (!isFullscreen) {
        setDomOutputRoot(null);
      }
    };
  }, [setDomOutputRoot, isFullscreen]);

  const openPopupManual = () => {
    let popup = null;
    try {
      popup = window.open('', 'waluau-preview');
    } catch (e) {
      console.warn('Failed to open preview window manually:', e);
    }
    if (popup) {
      setPopupBlocked(false);
      popupRef.current = popup;
      popup.document.open();
      popup.document.write(DOM_OUTPUT_SRC_DOC);
      popup.document.close();
      popup.focus();
      setDomOutputRoot(popup.document);

      const handleClose = () => {
        if (onCloseRef.current) {
          onCloseRef.current();
        }
      };
      popup.addEventListener('pagehide', handleClose);
    } else {
      alert('Could not open preview window. Please allow popups for this site.');
    }
  };

  if (isFullscreen) {
    if (popupBlocked) {
      return (
        <div className="dom-output-blocked">
          <span className="dom-output-blocked-icon">⚠️</span>
          <p className="dom-output-blocked-text">Popup window was blocked by the browser.</p>
          <button
            type="button"
            className="dom-output-open-popup-btn"
            onClick={openPopupManual}
          >
            Open Full Screen Window
          </button>
        </div>
      );
    }
    return (
      <div className="dom-output-placeholder">
        <span className="dom-output-placeholder-icon">🌐</span>
        <p className="dom-output-placeholder-text">Preview active in a separate fullscreen window</p>
      </div>
    );
  }

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
  requestAutoRun,
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


  // Escape key handling is skipped since proper window closure is handled by the browser/OS.

  useEffect(() => {
    if (!autoRun) return;
    for (const func of exportsList) {
      requestAutoRun(
        func.name,
        func.params,
        func.richParams,
        func.richReturns
      );
    }
  }, [autoRun, exportsList, funcInputs, requestAutoRun]);

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
        <section className="dom-output-section" aria-label="DOM Output">
          <div className="dom-output-header">
            <div className="dom-output-label">DOM Output</div>
            <button
              type="button"
              className="dom-output-fullscreen-btn"
              onClick={() => setIsFullscreen(!isFullscreen)}
              title={isFullscreen ? "Exit Full Screen" : "Full Screen"}
            >
              {isFullscreen ? (
                <>
                  <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                    <line x1="18" y1="6" x2="6" y2="18" />
                    <line x1="6" y1="6" x2="18" y2="18" />
                  </svg>
                  <span>Exit Full Screen</span>
                </>
              ) : (
                <>
                  <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                    <path d="M8 3H5a2 2 0 0 0-2 2v3m18 0V5a2 2 0 0 0-2-2h-3m0 18h3a2 2 0 0 0 2-2v-3M3 16v3a2 2 0 0 0 2 2h3" />
                  </svg>
                  <span>Full Screen</span>
                </>
              )}
            </button>
          </div>
          <DomOutputFrame
            setDomOutputRoot={setDomOutputRoot}
            onClose={() => setIsFullscreen(false)}
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
