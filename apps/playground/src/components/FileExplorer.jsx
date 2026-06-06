export default function FileExplorer({
  files,
  activeFile,
  setActiveFile,
  entryFile,
  editingFile,
  setEditingFile,
  editingValue,
  setEditingValue,
  handleAddFile,
  handleDeleteFile,
  handleRenameFile,
  handleSetEntryFile,
}) {
  return (
    <div className="file-explorer">
      <div className="explorer-header">
        <span>Files</span>
        <button
          className="explorer-btn-add"
          title="New File"
          onClick={() => {
            const name = prompt("Enter file name (e.g. math.walu):");
            if (name) handleAddFile(name);
          }}
        >
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
            <line x1="12" y1="5" x2="12" y2="19"></line>
            <line x1="5" y1="12" x2="19" y2="12"></line>
          </svg>
        </button>
      </div>
      <div className="file-list">
        {Object.keys(files).map((filename) => {
          const isActive = filename === activeFile;
          const isEntry = filename === entryFile;
          const displayName = filename.startsWith('/') ? filename.slice(1) : filename;

          return (
            <div
              key={filename}
              className={`file-item ${isActive ? 'active' : ''} ${isEntry ? 'entry' : ''}`}
              onClick={() => {
                setActiveFile(filename);
              }}
            >
              {editingFile === filename ? (
                <input
                  type="text"
                  className="file-edit-input"
                  value={editingValue}
                  autoFocus
                  onChange={(e) => setEditingValue(e.target.value)}
                  onBlur={() => {
                    if (editingValue.trim()) {
                      handleRenameFile(filename, editingValue);
                    }
                    setEditingFile(null);
                  }}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') {
                      if (editingValue.trim()) {
                        handleRenameFile(filename, editingValue);
                      }
                      setEditingFile(null);
                    } else if (e.key === 'Escape') {
                      setEditingFile(null);
                    }
                  }}
                  onClick={(e) => e.stopPropagation()}
                />
              ) : (
                <>
                  <span className="file-name-text" title={displayName}>
                    {displayName}
                  </span>
                  <div className="file-actions">
                    {!isEntry && (
                      <button
                        className="file-action-btn entry"
                        title="Set as Entry Point"
                        onClick={(e) => {
                          e.stopPropagation();
                          handleSetEntryFile(filename);
                        }}
                      >
                        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                          <polygon points="5 3 19 12 5 21 5 3"></polygon>
                        </svg>
                      </button>
                    )}
                    <button
                      className="file-action-btn rename"
                      title="Rename File"
                      onClick={(e) => {
                        e.stopPropagation();
                        setEditingFile(filename);
                        setEditingValue(displayName);
                      }}
                    >
                      <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                        <path d="M12 20h9"></path>
                        <path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z"></path>
                      </svg>
                    </button>
                    <button
                      className="file-action-btn delete"
                      title="Delete File"
                      onClick={(e) => {
                        e.stopPropagation();
                        handleDeleteFile(filename);
                      }}
                    >
                      <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
                        <polyline points="3 6 5 6 21 6"></polyline>
                        <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"></path>
                      </svg>
                    </button>
                  </div>
                </>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
