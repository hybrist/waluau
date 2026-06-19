import { useEffect, useRef, useState } from 'react';

export default function ReplTab({ ready, loadError, cells, busy, evaluate, reset }) {
  const [draft, setDraft] = useState('');
  const [historyIndex, setHistoryIndex] = useState(null);
  const transcriptRef = useRef(null);
  const inputRef = useRef(null);

  const submitted = cells.map((cell) => cell.input);

  // Keep the transcript pinned to the latest cell as it grows.
  useEffect(() => {
    const node = transcriptRef.current;
    if (node) node.scrollTop = node.scrollHeight;
  }, [cells]);

  const runDraft = async () => {
    const value = draft;
    if (!value.trim() || busy) return;
    setDraft('');
    setHistoryIndex(null);
    await evaluate(value);
    inputRef.current?.focus();
  };

  const handleKeyDown = (e) => {
    // Enter submits; Shift+Enter inserts a newline for multi-line cells.
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      runDraft();
      return;
    }
    // Up/Down walks submitted history when the caret is on the first/last line.
    if (e.key === 'ArrowUp' && !e.shiftKey && submitted.length > 0) {
      const onFirstLine = e.target.selectionStart <= (draft.indexOf('\n') === -1 ? draft.length : draft.indexOf('\n'));
      if (!onFirstLine && draft !== '') return;
      e.preventDefault();
      const next = historyIndex === null ? submitted.length - 1 : Math.max(0, historyIndex - 1);
      setHistoryIndex(next);
      setDraft(submitted[next]);
      return;
    }
    if (e.key === 'ArrowDown' && historyIndex !== null) {
      e.preventDefault();
      if (historyIndex >= submitted.length - 1) {
        setHistoryIndex(null);
        setDraft('');
      } else {
        const next = historyIndex + 1;
        setHistoryIndex(next);
        setDraft(submitted[next]);
      }
    }
  };

  if (loadError) {
    return (
      <div className="output-container">
        <div className="error-state">
          <h4>Compiler failed to load</h4>
          <pre className="diagnostic-output">{loadError}</pre>
        </div>
      </div>
    );
  }

  if (!ready) {
    return (
      <div className="output-container">
        <div className="loading-state">
          <div className="spinner"></div>
          <p>Initializing Waluau compiler module...</p>
        </div>
      </div>
    );
  }

  return (
    <div className="repl-container">
      <div className="repl-toolbar">
        <span className="repl-hint">
          Each cell is appended to the session and the whole program is recompiled. Use{' '}
          <code>print(...)</code> to see output. Enter runs, Shift+Enter for a new line.
        </span>
        <button className="repl-reset-btn" onClick={reset} disabled={cells.length === 0}>
          Reset session
        </button>
      </div>

      <div className="repl-transcript" ref={transcriptRef}>
        {cells.length === 0 && (
          <div className="repl-empty">
            Session is empty. Try{' '}
            <code>{'local name = "world"'}</code> then <code>{'print("hello " .. name)'}</code>.
          </div>
        )}
        {cells.map((cell, idx) => (
          <div className={`repl-cell ${cell.ok ? '' : 'repl-cell-error'}`} key={idx}>
            <div className="repl-cell-input">
              <span className="repl-prompt">&gt;</span>
              <pre className="repl-cell-source">{cell.input}</pre>
            </div>
            {cell.error ? (
              <pre className="repl-cell-output repl-output-error">{cell.error}</pre>
            ) : cell.output.length > 0 ? (
              <pre className="repl-cell-output">{cell.output.join('\n')}</pre>
            ) : (
              <div className="repl-cell-ok">ok</div>
            )}
          </div>
        ))}
      </div>

      <div className="repl-input-row">
        <span className="repl-prompt">&gt;</span>
        <textarea
          ref={inputRef}
          className="repl-input"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder='print("hello from the REPL")'
          rows={Math.min(8, Math.max(1, draft.split('\n').length))}
          spellCheck={false}
          autoComplete="off"
        />
        <button className="repl-run-btn" onClick={runDraft} disabled={busy || !draft.trim()}>
          {busy ? 'Running…' : 'Run'}
        </button>
      </div>
    </div>
  );
}
