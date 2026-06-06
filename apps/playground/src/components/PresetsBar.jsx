export default function PresetsBar({ presets, selectPreset, setOpenFileSearch }) {
  return (
    <div className="presets-bar">
      <span className="presets-label">Examples:</span>
      <button
        className="preset-btn search-trigger-btn"
        title="Search fixture & conformance files (Cmd+P / Ctrl+P)"
        onClick={() => setOpenFileSearch(true)}
        style={{
          display: 'inline-flex',
          alignItems: 'center',
          gap: '4px',
          background: 'rgba(55, 148, 255, 0.1)',
          borderColor: 'rgba(55, 148, 255, 0.3)',
          color: 'var(--accent-cyan)',
          fontWeight: '600'
        }}
      >
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" style={{ marginRight: '2px' }}>
          <circle cx="11" cy="11" r="8"></circle>
          <line x1="21" y1="21" x2="16.65" y2="16.65"></line>
        </svg>
        Search (Cmd+P)
      </button>
      {presets.map((preset) => (
        <button key={preset.key} className="preset-btn" onClick={() => selectPreset(preset)}>
          {preset.label}
        </button>
      ))}
    </div>
  );
}
