import { getDefaultParamValue, renderType } from '../utils/wasm.js';
import { ParamField } from './ParamFields.jsx';

export default function RunTab({
  status,
  runError,
  exportsList,
  funcInputs,
  initLogs,
  autoRun,
  setAutoRun,
  handleInputChange,
  handleRecordFieldChange,
  handleManualRun,
  getResult,
}) {
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
