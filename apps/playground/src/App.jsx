import { useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import Editor from '@monaco-editor/react';

import FileTabs from './components/FileTabs.jsx';
import FileSearchModal from './components/FileSearchModal.jsx';
import InlineRunner from './components/InlineRunner.jsx';
import RunTab from './components/RunTab.jsx';
import ReplTab from './components/ReplTab.jsx';
import PresetsBar from './components/PresetsBar.jsx';

import { PRESETS } from './utils/presets.js';
import useFiles from './hooks/useFiles.js';
import useMonacoEditor from './hooks/useMonacoEditor.js';
import useWaluauCompiler from './hooks/useWaluauCompiler.js';
import useWaluauRepl from './hooks/useWaluauRepl.js';

export default function App() {
  const [activeTab, setActiveTab] = useState('run'); // 'run', 'ir', 'wat', 'logs'
  const [openFileSearch, setOpenFileSearch] = useState(false);

  // Hook 1: Files state & operations management
  const {
    files,
    activeFile,
    setActiveFile,
    entryFile,
    assetManifest,
    editingFile,
    setEditingFile,
    editingValue,
    setEditingValue,
    selectPreset,
    handleAddFile,
    handleDeleteFile,
    handleRenameFile,
    handleFileChange,
    handleSetEntryFile,
    allSearchableFiles
  } = useFiles();

  // Hook 2: Compiler & runner state management
  const {
    status,
    outputIr,
    outputWat,
    outputWasmBytes,
    displayStatus,
    errorMsg,
    diagnostics,
    sendLspRequest,
    runError,
    exportsList,
    initLogs,
    usesDomOutput,
    setDomOutputRoot,
    funcInputs,
    autoRun,
    setAutoRun,
    handleInputChange,
    handleRecordFieldChange,
    handleManualRun,
    requestAutoRun,
    getResult
  } = useWaluauCompiler({
    files,
    entryFile,
    assetManifest,
  });

  // Hook 2b: Standalone REPL session (accumulate-and-recompile), optionally
  // seeded from the current editor program.
  const repl = useWaluauRepl();

  // Auto-seed the REPL from the editor the first time the tab is opened.
  const { ready: replReady, maybeAutoSeed: replMaybeAutoSeed } = repl;
  useEffect(() => {
    if (activeTab === 'repl' && replReady) {
      replMaybeAutoSeed(files, entryFile);
    }
  }, [activeTab, replReady, replMaybeAutoSeed, files, entryFile]);

  // Hook 3: Monaco Editor wrapper logic (view zones, markers, model disposal)
  const {
    handleEditorBeforeMount,
    handleEditorDidMount,
    activeRunners,
    removeRunnerZone
  } = useMonacoEditor({
    files,
    entryFile,
    exportsList,
    outputWasmBytes,
    errorMsg,
    diagnostics,
    sendLspRequest
  });

  // Hotkey listener for Quick Open File Search modal
  useEffect(() => {
    const handleGlobalKeyDown = (e) => {
      const isTrigger = (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'p';
      if (isTrigger) {
        e.preventDefault();
        e.stopPropagation();
        setOpenFileSearch((prev) => !prev);
      }
    };
    window.addEventListener('keydown', handleGlobalKeyDown, true);
    return () => {
      window.removeEventListener('keydown', handleGlobalKeyDown, true);
    };
  }, []);

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
          <div className={`status-badge ${displayStatus}`}>
            <span className="pulse-dot"></span>
            <span className="status-text">
              {displayStatus === 'loading' && 'Loading Compiler...'}
              {displayStatus === 'analyzing' && 'Analyzing...'}
              {displayStatus === 'ready' && 'Ready'}
              {displayStatus === 'success' && 'Compilation Succeeded'}
              {displayStatus === 'error' && 'Compilation Failed'}
            </span>
          </div>
        </div>
      </header>

      {/* Preset toolbar */}
      <PresetsBar
        presets={PRESETS}
        selectPreset={selectPreset}
        setOpenFileSearch={setOpenFileSearch}
      />

      <main className="playground-main">
        {/* Editor Column */}
        <section className="column-panel editor-panel">
          <div className="panel-header tab-header">
            <FileTabs
              files={files}
              activeFile={activeFile}
              setActiveFile={setActiveFile}
              entryFile={entryFile}
              editingFile={editingFile}
              setEditingFile={setEditingFile}
              editingValue={editingValue}
              setEditingValue={setEditingValue}
              handleAddFile={handleAddFile}
              handleDeleteFile={handleDeleteFile}
              handleRenameFile={handleRenameFile}
              handleSetEntryFile={handleSetEntryFile}
            />
          </div>
          <div className="editor-layout">
            {/* Monaco Editor Container */}
            <div className="editor-container">
              <div style={{ flex: 1, height: '100%' }}>
                <Editor
                  height="100%"
                  theme="vs-dark"
                  language="waluau"
                  path={activeFile}
                  value={files[activeFile] ?? ''}
                  onChange={(value) => handleFileChange(activeFile, value ?? '')}
                  beforeMount={handleEditorBeforeMount}
                  onMount={handleEditorDidMount}
                  options={{
                    minimap: { enabled: false },
                    fontSize: 14,
                    fontFamily: 'var(--font-mono)',
                    lineHeight: 1.6,
                    padding: { top: 16, bottom: 16 },
                    scrollBeyondLastLine: false,
                    automaticLayout: true,
                  }}
                />
              </div>
              {/* Visually hidden but active textarea for Playwright test compatibility */}
              <textarea
                className="code-textarea"
                value={files[activeFile] ?? ''}
                onChange={(e) => handleFileChange(activeFile, e.target.value)}
                style={{
                  position: 'absolute',
                  left: '-9999px',
                  top: '-9999px',
                  width: '100px',
                  height: '100px',
                }}
              />
            </div>
          </div>
        </section>

        {/* Output Column */}
        <section className="column-panel output-panel">
          <div className="panel-header tab-header">
            <div className="tab-buttons">
              <button
                className={`tab-btn ${activeTab === 'run' ? 'active' : ''}`}
                onClick={() => setActiveTab('run')}
              >
                Run
              </button>
              <button
                className={`tab-btn ${activeTab === 'repl' ? 'active' : ''}`}
                onClick={() => setActiveTab('repl')}
              >
                REPL
              </button>
              <button
                className={`tab-btn ${activeTab === 'ir' ? 'active' : ''}`}
                onClick={() => setActiveTab('ir')}
              >
                Generated IR
              </button>
              <button
                className={`tab-btn ${activeTab === 'wat' ? 'active' : ''}`}
                onClick={() => setActiveTab('wat')}
              >
                Wasm Text (WAT)
              </button>
              <button
                className={`tab-btn ${activeTab === 'logs' ? 'active' : ''}`}
                onClick={() => setActiveTab('logs')}
              >
                Compiler Diagnostics
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
                ) : displayStatus === 'error' && !outputIr ? (
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
                    <code>{outputIr || '-- No IR output generated. Write some valid code first.'}</code>
                  </pre>
                )}
              </div>
            )}

            {activeTab === 'wat' && (
              <div className="output-container">
                {status === 'loading' ? (
                  <div className="loading-state">
                    <div className="spinner"></div>
                    <p>Initializing Waluau compiler module...</p>
                  </div>
                ) : displayStatus === 'error' && !outputWat ? (
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
                  <pre className="wat-output">
                    <code>{outputWat || '-- No WAT output generated. Write some valid code first.'}</code>
                  </pre>
                )}
              </div>
            )}

            {activeTab === 'logs' && (
              <div className="logs-container">
                <h3>Compilation Log</h3>
                 <div className={`log-card ${displayStatus}`}>
                  <div className="log-header">
                    <span className="log-bullet"></span>
                    <span className="log-title">
                      {displayStatus === 'success' && 'Build Succeeded'}
                      {displayStatus === 'error' && 'Build Failed'}
                      {displayStatus === 'ready' && 'Ready'}
                      {displayStatus === 'loading' && 'Loading...'}
                    </span>
                  </div>
                  <div className="log-body">
                    {displayStatus === 'success' && (
                      <p className="success-text">
                        The program parsed and type-checked successfully! The intermediate representation was constructed and verified without errors.
                      </p>
                    )}
                    {displayStatus === 'error' && (
                      <div className="error-details">
                        <p className="failure-text">Compiler diagnostics returned the following error:</p>
                        <pre className="error-block">{errorMsg}</pre>
                      </div>
                    )}
                    {displayStatus === 'ready' && <p>No compilation has run yet. Enter code to trigger compiler.</p>}
                  </div>
                </div>
              </div>
            )}

            {activeTab === 'repl' && (
              <ReplTab
                ready={repl.ready}
                loadError={repl.loadError}
                cells={repl.cells}
                busy={repl.busy}
                evaluate={repl.evaluate}
                reset={repl.reset}
                onLoadScript={() => repl.seed(files, entryFile)}
                scriptName={entryFile.replace(/^\//, '')}
              />
            )}

            {activeTab === 'run' && (
              <RunTab
                status={status}
                runError={runError}
                exportsList={exportsList}
                funcInputs={funcInputs}
                initLogs={initLogs}
                usesDomOutput={usesDomOutput}
                setDomOutputRoot={setDomOutputRoot}
                autoRun={autoRun}
                setAutoRun={setAutoRun}
                handleInputChange={handleInputChange}
                handleRecordFieldChange={handleRecordFieldChange}
                handleManualRun={handleManualRun}
                requestAutoRun={requestAutoRun}
                getResult={getResult}
              />
            )}
          </div>
        </section>
      </main>

      {/* Render inline runners in their respective view zones via React Portals */}
      {activeRunners.map((runner) =>
        createPortal(
          <InlineRunner
            key={runner.id}
            funcName={runner.funcName}
            exportsList={exportsList}
            funcInputs={funcInputs}
            handleInputChange={handleInputChange}
            handleRecordFieldChange={handleRecordFieldChange}
            handleManualRun={handleManualRun}
            requestAutoRun={requestAutoRun}
            getResult={getResult}
            autoRun={autoRun}
            onClose={() => removeRunnerZone(runner)}
          />,
          runner.domNode
        )
      )}
      {/* File Search Modal */}
      {openFileSearch && (
        <FileSearchModal
          isOpen={openFileSearch}
          onClose={() => setOpenFileSearch(false)}
          items={allSearchableFiles}
        />
      )}
    </div>
  );
}
