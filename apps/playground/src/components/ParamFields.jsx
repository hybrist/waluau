import { getEntries, renderType, getDefaultParamValue } from '../utils/wasm.js';

function RecordInputFields({ funcName, paramIdx, type, currentVal, handleRecordFieldChange, handleInputChange, path = [], isInline = false }) {
  return getEntries(type.value.fields).map(([fieldName, fieldTy]) => {
    const fieldPath = [...path, fieldName];
    const val = currentVal ? currentVal[fieldName] : getDefaultParamValue(fieldTy);

    if (fieldTy.kind === 'Record') {
      return (
        <div key={fieldName} style={{ marginLeft: isInline ? '4px' : '12px', marginTop: '6px', borderLeft: '2px solid rgba(255,255,255,0.1)', paddingLeft: '8px' }}>
          <div style={{ fontSize: '12px', opacity: 0.7, fontWeight: 'bold' }}>{fieldName}:</div>
          <RecordInputFields
            funcName={funcName}
            paramIdx={paramIdx}
            type={fieldTy}
            currentVal={val}
            handleRecordFieldChange={handleRecordFieldChange}
            handleInputChange={handleInputChange}
            path={fieldPath}
            isInline={isInline}
          />
        </div>
      );
    }

    if (fieldTy.kind === 'TaggedUnion') {
      return (
        <div key={fieldName} style={{ marginLeft: isInline ? '4px' : '12px', marginTop: '6px', borderLeft: '2px solid rgba(255,255,255,0.1)', paddingLeft: '8px' }}>
          <div style={{ fontSize: '12px', opacity: 0.7, fontWeight: 'bold' }}>{fieldName} (union):</div>
          <UnionInputFields
            funcName={funcName}
            paramIdx={paramIdx}
            type={fieldTy}
            currentVal={val}
            handleRecordFieldChange={handleRecordFieldChange}
            handleInputChange={handleInputChange}
            path={fieldPath}
            isInline={isInline}
          />
        </div>
      );
    }

    const typeLabel = renderType(fieldTy);
    return (
      <div key={fieldName} className={isInline ? "inline-runner-input-row" : "func-input-row"} style={{ marginLeft: '12px', marginTop: '4px' }}>
        <label className={isInline ? "inline-runner-label" : "func-input-label"}>{fieldName} ({typeLabel}):</label>
        <input
          type="text"
          className={isInline ? "inline-runner-field" : "func-input-field"}
          value={val ?? '0'}
          onChange={(e) => handleRecordFieldChange(funcName, paramIdx, fieldPath, e.target.value)}
          placeholder={`Enter ${typeLabel} value`}
        />
      </div>
    );
  });
}

function UnionInputFields({ funcName, paramIdx, type, currentVal, handleRecordFieldChange, handleInputChange, path = [], isInline = false }) {
  const selectedTag = currentVal?.tag || type.value.variants[0]?.tag;
  const selectedVariant = type.value.variants.find(v => v.tag === selectedTag);
  const payloadTy = selectedVariant ? selectedVariant.payload : null;
  const payloadVal = currentVal ? currentVal.value : (payloadTy ? getDefaultParamValue(payloadTy) : null);

  const handleTagChange = (newTag) => {
    const newVariant = type.value.variants.find(v => v.tag === newTag);
    const newPayloadVal = newVariant ? getDefaultParamValue(newVariant.payload) : null;
    const newVal = { tag: newTag, value: newPayloadVal };
    if (path.length === 0) {
      handleInputChange(funcName, paramIdx, newVal);
    } else {
      handleRecordFieldChange(funcName, paramIdx, path, newVal);
    }
  };

  return (
    <div style={{ marginLeft: isInline ? '4px' : '12px', marginTop: '6px', borderLeft: '2px solid rgba(255,255,255,0.1)', paddingLeft: '8px' }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: '8px', marginBottom: '4px' }}>
        <span style={{ fontSize: '12px', opacity: 0.7 }}>Variant:</span>
        <select
          value={selectedTag}
          onChange={(e) => handleTagChange(e.target.value)}
          className={isInline ? "inline-runner-field" : "func-input-field"}
          style={{ width: 'auto', minWidth: '100px', padding: '2px 6px', height: '26px' }}
        >
          {type.value.variants.map(v => (
            <option key={v.tag} value={v.tag}>{v.tag}</option>
          ))}
        </select>
      </div>
      {payloadTy && payloadTy.kind !== 'Unit' && (
        <div style={{ marginTop: '4px' }}>
          <div style={{ fontSize: '12px', opacity: 0.7, fontWeight: 'bold' }}>payload:</div>
          {payloadTy.kind === 'Record' ? (
            <RecordInputFields
              funcName={funcName}
              paramIdx={paramIdx}
              type={payloadTy}
              currentVal={payloadVal}
              handleRecordFieldChange={handleRecordFieldChange}
              handleInputChange={handleInputChange}
              path={[...path, 'value']}
              isInline={isInline}
            />
          ) : payloadTy.kind === 'TaggedUnion' ? (
            <UnionInputFields
              funcName={funcName}
              paramIdx={paramIdx}
              type={payloadTy}
              currentVal={payloadVal}
              handleRecordFieldChange={handleRecordFieldChange}
              handleInputChange={handleInputChange}
              path={[...path, 'value']}
              isInline={isInline}
            />
          ) : (
            <div className={isInline ? "inline-runner-input-row" : "func-input-row"} style={{ marginLeft: '12px', marginTop: '4px' }}>
              <input
                type="text"
                className={isInline ? "inline-runner-field" : "func-input-field"}
                value={payloadVal ?? ''}
                onChange={(e) => {
                  if (path.length === 0) {
                    handleInputChange(funcName, paramIdx, { tag: selectedTag, value: e.target.value });
                  } else {
                    handleRecordFieldChange(funcName, paramIdx, [...path, 'value'], e.target.value);
                  }
                }}
                placeholder={`Enter ${renderType(payloadTy)}`}
              />
            </div>
          )}
        </div>
      )}
    </div>
  );
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
          handleInputChange={handleInputChange}
          isInline={isInline}
        />
      </div>
    );
  }

  if (richType && richType.kind === 'TaggedUnion') {
    return (
      <div key={paramIdx} style={{ marginTop: '8px', marginBottom: '8px' }}>
        <div style={{ fontSize: '13px', fontWeight: 'bold', color: 'var(--accent-cyan)' }}>param{paramIdx} (union):</div>
        <UnionInputFields
          funcName={funcName}
          paramIdx={paramIdx}
          type={richType}
          currentVal={val}
          handleRecordFieldChange={handleRecordFieldChange}
          handleInputChange={handleInputChange}
          isInline={isInline}
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
