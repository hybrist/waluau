import { getDefaultParamValue, renderType } from '../utils/wasm.js';
import { ParamField } from './ParamFields.jsx';

export default function InlineRunner({
  funcName,
  exportsList,
  funcInputs,
  handleInputChange,
  handleRecordFieldChange,
  handleManualRun,
  getResult,
  autoRun,
  onClose,
}) {
  const func = exportsList.find((e) => e.name === funcName);
  if (!func) return null;

  const inputs = funcInputs[funcName] || (func.richParams || func.params).map(getDefaultParamValue);
  const res = getResult(funcName, func.params, func.richParams, func.richReturns);

  return (
    <div className="inline-runner-widget">
      <div className="inline-runner-header">
        <div className="inline-runner-signature">
          <span className="inline-runner-name">{func.name}</span>
          <span>(</span>
          {func.richParams ? (
            func.richParams.map((type, idx) => (
              <span key={idx}>
                param{idx}: <span className="inline-runner-type">{renderType(type)}</span>
                {idx < func.richParams.length - 1 ? ', ' : ''}
              </span>
            ))
          ) : (
            func.params.map((type, idx) => (
              <span key={idx}>
                param{idx}: <span className="inline-runner-type">{type}</span>
                {idx < func.params.length - 1 ? ', ' : ''}
              </span>
            ))
          )}
          <span>)</span>
          <span className="inline-runner-arrow"> -&gt; </span>
          <span className="inline-runner-type">
            {func.richReturns ? (
              func.richReturns.length > 0 ? func.richReturns.map(renderType).join(', ') : 'void'
            ) : (
              func.returns.length > 0 ? func.returns.join(', ') : 'void'
            )}
          </span>
        </div>
        <button className="inline-runner-close" onClick={onClose} title="Close inline runner">
          &times;
        </button>
      </div>

      {func.params.length > 0 && (
        <div className="inline-runner-inputs">
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
                isInline={true}
              />
            );
          })}
        </div>
      )}

      <div className="inline-runner-actions">
        {!autoRun && (
          <button
            className="inline-runner-run-btn"
            onClick={() => handleManualRun(func.name, func.params, func.richParams, func.richReturns)}
          >
            Run
          </button>
        )}
        <div className={`inline-runner-result ${res.error ? 'error' : res.isIdle ? '' : 'success'}`}>
          <span className="result-label">Result:</span>
          {res.isIdle ? (
            <span className="result-value idle">Click Run</span>
          ) : res.error ? (
            <span className="result-value error">{res.error}</span>
          ) : (
            <span className="result-value success">{res.value}</span>
          )}
        </div>
      </div>
    </div>
  );
}
