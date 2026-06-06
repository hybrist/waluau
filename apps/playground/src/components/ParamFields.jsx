import { getEntries, renderType, getDefaultParamValue } from '../utils/wasm.js';

function RecordInputFields({ funcName, paramIdx, type, currentVal, handleRecordFieldChange, path = [] }) {
  return getEntries(type.value.fields).map(([fieldName, fieldTy]) => {
    const fieldPath = [...path, fieldName];
    const val = currentVal ? currentVal[fieldName] : getDefaultParamValue(fieldTy);

    if (fieldTy.kind === 'Record') {
      return (
        <div key={fieldName} style={{ marginLeft: '12px', marginTop: '6px', borderLeft: '2px solid rgba(255,255,255,0.1)', paddingLeft: '8px' }}>
          <div style={{ fontSize: '12px', opacity: 0.7, fontWeight: 'bold' }}>{fieldName}:</div>
          <RecordInputFields
            funcName={funcName}
            paramIdx={paramIdx}
            type={fieldTy}
            currentVal={val}
            handleRecordFieldChange={handleRecordFieldChange}
            path={fieldPath}
          />
        </div>
      );
    }

    const typeLabel = renderType(fieldTy);
    return (
      <div key={fieldName} className="func-input-row" style={{ marginLeft: '12px', marginTop: '4px' }}>
        <label className="func-input-label">{fieldName} ({typeLabel}):</label>
        <input
          type="text"
          className="func-input-field"
          value={val ?? '0'}
          onChange={(e) => handleRecordFieldChange(funcName, paramIdx, fieldPath, e.target.value)}
          placeholder={`Enter ${typeLabel} value`}
        />
      </div>
    );
  });
}

export function ParamField({ funcName, paramIdx, type, richType, val, handleInputChange, handleRecordFieldChange, isInline = false }) {
  if (richType && richType.kind === 'Record') {
    return (
      <div key={paramIdx} style={{ marginTop: '8px', marginBottom: '8px' }}>
        <div style={{ fontSize: '13px', fontWeight: 'bold', color: 'var(--accent-cyan)' }}>param{paramIdx} (record):</div>
        <RecordInputFields
          funcName={funcName}
          paramIdx={paramIdx}
          type={richType}
          currentVal={val}
          handleRecordFieldChange={handleRecordFieldChange}
        />
      </div>
    );
  }

  const typeLabel = richType ? renderType(richType) : type;
  return (
    <div key={paramIdx} className={isInline ? "inline-runner-input-row" : "func-input-row"}>
      <label className={isInline ? "inline-runner-label" : "func-input-label"}>param{paramIdx} ({typeLabel}):</label>
      <input
        type="text"
        className={isInline ? "inline-runner-field" : "func-input-field"}
        value={val ?? '0'}
        onChange={(e) => handleInputChange(funcName, paramIdx, e.target.value)}
        placeholder={`Enter ${typeLabel} value`}
      />
    </div>
  );
}
