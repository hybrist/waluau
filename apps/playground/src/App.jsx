import { useEffect, useMemo, useState, useRef } from 'react';
import { createPortal } from 'react-dom';
import Editor from '@monaco-editor/react';

import {
  WALUAU_STRING_CONSTANTS_MODULE,
  buildWaluauImports,
  getWasmExports,
  getDefaultParamValue,
  executeCall,
  classifyWasmInstantiationError
} from './utils/wasm.js';

import FileExplorer from './components/FileExplorer.jsx';
import FileSearchModal from './components/FileSearchModal.jsx';
import InlineRunner from './components/InlineRunner.jsx';
import RunTab from './components/RunTab.jsx';
import PresetsBar from './components/PresetsBar.jsx';

const fixtureModules = import.meta.glob('../../../fixtures/*.walu', {
  eager: true,
  query: '?raw',
  import: 'default'
});

const moduleFixtures = import.meta.glob('../../../fixtures/modules/*.walu', {
  eager: true,
  query: '?raw',
  import: 'default'
});

const conformanceModules = import.meta.glob('../../../conformance/*.walu', {
  eager: true,
  query: '?raw',
  import: 'default'
});

const SINGLE_PRESETS = Object.entries(fixtureModules)
  .map(([path, source]) => {
    const filename = path.split('/').pop();
    const key = filename.replace(/\.walu$/, '');
    const label = key
      .split('-')
      .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
      .join(' ');
    
    return {
      key,
      label,
      files: {
        [`/${filename}`]: source
      },
      entryFile: `/${filename}`
    };
  });

const CONFORMANCE_PRESETS = Object.entries(conformanceModules)
  .map(([path, source]) => {
    const filename = path.split('/').pop();
    const key = filename.replace(/\.walu$/, '');
    const label = key
      .split('_')
      .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
      .join(' ');
    
    return {
      key: `conformance-${key}`,
      label: `${label} (Test)`,
      files: {
        [`/${filename}`]: source
      },
      entryFile: `/${filename}`
    };
  });

const MULTI_PRESET = {
  key: 'require-flow',
  label: 'Require Flow Example',
  files: Object.entries(moduleFixtures).reduce((acc, [path, source]) => {
    const filename = path.split('/').pop();
    acc[`/${filename}`] = source;
    return acc;
  }, {}),
  entryFile: '/main.walu'
};

const PRESETS = [...SINGLE_PRESETS, MULTI_PRESET, ...CONFORMANCE_PRESETS].sort((left, right) =>
  left.label.localeCompare(right.label)
);

const DEFAULT_PRESET = PRESETS[0] || {
  key: 'default',
  label: 'Default',
  files: { '/main.walu': '' },
  entryFile: '/main.walu'
};

export default function App() {
  const [files, setFiles] = useState(DEFAULT_PRESET.files);
  const [activeFile, setActiveFile] = useState(DEFAULT_PRESET.entryFile);
  const [entryFile, setEntryFile] = useState(DEFAULT_PRESET.entryFile);
  const [editingFile, setEditingFile] = useState(null);
  const [editingValue, setEditingValue] = useState('');
  const [status, setStatus] = useState('loading'); // 'loading', 'ready', 'success', 'error'
  const [loadErrorMsg, setLoadErrorMsg] = useState('');
  const [compilerReady, setCompilerReady] = useState(false);
  const [compileSource, setCompileSource] = useState(null);
  const [activeTab, setActiveTab] = useState('run'); // 'run', 'ir', 'wat', 'logs'
  const [runInstance, setRunInstance] = useState(null);
  const [runError, setRunError] = useState(null);
  const [exportsList, setExportsList] = useState([]);
  const [initLogs, setInitLogs] = useState([]);
  const [funcInputs, setFuncInputs] = useState({});
  const [autoRun, setAutoRun] = useState(true);
  const [manualResults, setManualResults] = useState({});
  const [editorInstance, setEditorInstance] = useState(null);
  const [monacoInstance, setMonacoInstance] = useState(null);
  const [activeRunners, setActiveRunners] = useState([]);
  const [openFileSearch, setOpenFileSearch] = useState(false);

  const selectPreset = (preset) => {
    setFiles(preset.files);
    setActiveFile(preset.entryFile);
    setEntryFile(preset.entryFile);
  };

  const allSearchableFiles = useMemo(() => {
    const items = [];
    
    // 1. Single fixtures
    for (const [path, source] of Object.entries(fixtureModules)) {
      const filename = path.split('/').pop();
      items.push({
        type: 'fixture',
        path,
        name: filename,
        source,
        category: 'Fixture',
        onSelect: () => {
          selectPreset({
            key: filename.replace(/\.walu$/, ''),
            files: { [`/${filename}`]: source },
            entryFile: `/${filename}`
          });
        }
      });
    }

    // 2. Conformance tests
    for (const [path, source] of Object.entries(conformanceModules)) {
      const filename = path.split('/').pop();
      items.push({
        type: 'conformance',
        path,
        name: filename,
        source,
        category: 'Conformance',
        onSelect: () => {
          selectPreset({
            key: `conformance-${filename.replace(/\.walu$/, '')}`,
            files: { [`/${filename}`]: source },
            entryFile: `/${filename}`
          });
        }
      });
    }

    // 3. Module fixtures
    for (const [path, source] of Object.entries(moduleFixtures)) {
      const filename = path.split('/').pop();
      items.push({
        type: 'module',
        path,
        name: `modules/${filename}`,
        source,
        category: 'Module',
        onSelect: () => {
          setFiles(MULTI_PRESET.files);
          setEntryFile(MULTI_PRESET.entryFile);
          setActiveFile(`/${filename}`);
        }
      });
    }

    return items.sort((left, right) => left.name.localeCompare(right.name));
  }, []);

  useEffect(() => {
    const handleGlobalKeyDown = (e) => {
      const isTrigger = (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'p';
      if (isTrigger) {
        e.preventDefault();
        e.stopPropagation();
        setOpenFileSearch(prev => !prev);
      }
    };
    window.addEventListener('keydown', handleGlobalKeyDown, true);
    return () => {
      window.removeEventListener('keydown', handleGlobalKeyDown, true);
    };
  }, []);

  const exportsListRef = useRef(exportsList);
  const activeRunnersRef = useRef(activeRunners);
  const toggleInlineRunnerRef = useRef(null);

  useEffect(() => {
    exportsListRef.current = exportsList;
  }, [exportsList]);

  useEffect(() => {
    activeRunnersRef.current = activeRunners;
  }, [activeRunners]);

  const handleEditorBeforeMount = (monaco) => {
    if (!monaco.languages.getLanguages().some((lang) => lang.id === 'waluau')) {
      monaco.languages.register({ id: 'waluau' });

      monaco.languages.setMonarchTokensProvider('waluau', {
        defaultToken: '',
        tokenPostfix: '.waluau',

        keywords: [
          'and',
          'const',
          'do',
          'else',
          'elseif',
          'end',
          'false',
          'function',
          'if',
          'local',
          'not',
          'or',
          'repeat',
          'return',
          'then',
          'true',
          'until',
          'while',
        ],

        typeKeywords: ['number', 'u32', 'u64', 'i32', 'i64', 'f32', 'f64', 'bool', 'string'],

        brackets: [
          { token: 'delimiter.bracket', open: '{', close: '}' },
          { token: 'delimiter.array', open: '[', close: ']' },
          { token: 'delimiter.parenthesis', open: '(', close: ')' },
        ],

        operators: ['=', '==', '+=', '+', '-', '*', '/', '//', '%', '<', '>', '->', '::', '#'],

        symbols: /[=-><!~?:&|+*/^%#]+/,

        tokenizer: {
          root: [
            // identifiers and keywords
            [
              /[a-zA-Z_]\w*/,
              {
                cases: {
                  '@keywords': 'keyword',
                  '@typeKeywords': 'type',
                  '@default': 'identifier',
                },
              },
            ],

            // whitespace and comments
            { include: '@whitespace' },

            // delimiters and operators
            [/[{}()[\]]/, '@brackets'],

            [
              /@symbols/,
              {
                cases: {
                  '@operators': 'operator',
                  '@default': '',
                },
              },
            ],

            // numbers
            [/\d*\.\d+([eE][-+]?\d+)?/, 'number.float'],
            [/\d+/, 'number'],

            // delimiter: colon for type annotation, comma
            [/:/, 'delimiter'],
            [/,/, 'delimiter'],

            // strings
            [/"([^"\\]|\\.)*"/, 'string'],
          ],

          whitespace: [
            [/[ \t\r\n]+/, 'white'],
            [/--\[\[/, 'comment', '@comment'],
            [/--.*$/, 'comment'],
          ],

          comment: [
            [/[^\]]+/, 'comment'],
            [/\]\]/, 'comment', '@pop'],
            [/./, 'comment'],
          ],
        },
      });

      monaco.languages.setLanguageConfiguration('waluau', {
        comments: {
          lineComment: '--',
          blockComment: ['--[[', ']]'],
        },
        brackets: [
          ['{', '}'],
          ['[', ']'],
          ['(', ')'],
        ],
        autoClosingPairs: [
          { open: '{', close: '}' },
          { open: '[', close: ']' },
          { open: '(', close: ')' },
          { open: '"', close: '"' },
        ],
        surroundingPairs: [
          { open: '{', close: '}' },
          { open: '[', close: ']' },
          { open: '(', close: ')' },
          { open: '"', close: '"' },
        ],
      });
    }
  };

  const handleEditorDidMount = (editor, monaco) => {
    setEditorInstance(editor);
    setMonacoInstance(monaco);
  };

  const disposeModel = (filename) => {
    if (!monacoInstance) return;
    const models = monacoInstance.editor.getModels();
    const model = models.find(m => m.uri.path === filename || m.uri.toString().endsWith(filename));
    if (model) {
      model.dispose();
    }
  };

  const handleAddFile = (name) => {
    let filename = name.trim();
    if (!filename) return;
    if (!filename.startsWith('/')) {
      filename = '/' + filename;
    }
    if (!filename.endsWith('.walu')) {
      filename = filename + '.walu';
    }
    if (files[filename] !== undefined) {
      alert('File already exists');
      return;
    }
    setFiles(prev => ({
      ...prev,
      [filename]: ''
    }));
    setActiveFile(filename);
  };

  const handleDeleteFile = (filename) => {
    const fileKeys = Object.keys(files);
    if (fileKeys.length <= 1) {
      alert('Cannot delete the last remaining file');
      return;
    }
    if (confirm(`Are you sure you want to delete ${filename}?`)) {
      disposeModel(filename);
      setFiles(prev => {
        const next = { ...prev };
        delete next[filename];
        return next;
      });
      if (activeFile === filename) {
        const remaining = fileKeys.filter(f => f !== filename);
        setActiveFile(remaining[0]);
      }
      if (entryFile === filename) {
        const remaining = fileKeys.filter(f => f !== filename);
        setEntryFile(remaining[0]);
      }
    }
  };

  const handleRenameFile = (oldName, newName) => {
    let filename = newName.trim();
    if (!filename) return;
    if (!filename.startsWith('/')) {
      filename = '/' + filename;
    }
    if (!filename.endsWith('.walu')) {
      filename = filename + '.walu';
    }
    if (filename === oldName) return;
    if (files[filename] !== undefined) {
      alert('File already exists');
      return;
    }
    disposeModel(oldName);
    setFiles(prev => {
      const next = { ...prev };
      next[filename] = next[oldName];
      delete next[oldName];
      return next;
    });
    if (activeFile === oldName) {
      setActiveFile(filename);
    }
    if (entryFile === oldName) {
      setEntryFile(filename);
    }
  };

  const handleFileChange = (filename, value) => {
    setFiles(prev => ({
      ...prev,
      [filename]: value
    }));
  };

  const handleSetEntryFile = (filename) => {
    setEntryFile(filename);
  };

  const toggleInlineRunner = (funcName, lineNumber) => {
    const existingIndex = activeRunnersRef.current.findIndex(
      (r) => r.funcName === funcName && r.lineNumber === lineNumber
    );

    if (existingIndex !== -1) {
      const runner = activeRunnersRef.current[existingIndex];
      removeRunnerZone(runner);
    } else {
      addRunnerZone(funcName, lineNumber);
    }
  };

  const addRunnerZone = (funcName, lineNumber) => {
    if (!editorInstance) return;

    const domNode = document.createElement('div');
    domNode.className = 'inline-runner-zone';
    
    // Stop propagation of events to prevent Monaco from stealing focus/interaction
    const stopProp = (e) => e.stopPropagation();
    domNode.addEventListener('mousedown', stopProp);
    domNode.addEventListener('mouseup', stopProp);
    domNode.addEventListener('click', stopProp);
    domNode.addEventListener('keydown', stopProp);
    domNode.addEventListener('keyup', stopProp);

    const funcMeta = exportsListRef.current.find(e => e.name === funcName);
    const paramCount = funcMeta ? funcMeta.params.length : 0;
    // Height formula: base 5 lines, + 1.5 lines per parameter
    const heightInLines = Math.max(5, 4 + Math.ceil(paramCount * 1.5));

    let zoneId = null;
    editorInstance.changeViewZones((changeAccessor) => {
      zoneId = changeAccessor.addZone({
        afterLineNumber: lineNumber,
        heightInLines: heightInLines,
        domNode: domNode,
        suppressMouseDown: true
      });
    });

    const newRunner = {
      id: `${funcName}-${lineNumber}-${Date.now()}`,
      funcName,
      lineNumber,
      domNode,
      zoneId,
      heightInLines
    };

    setActiveRunners(prev => [...prev, newRunner]);
  };

  const removeRunnerZone = (runner) => {
    if (editorInstance && runner.zoneId) {
      editorInstance.changeViewZones((changeAccessor) => {
        changeAccessor.removeZone(runner.zoneId);
      });
    }
    setActiveRunners(prev => prev.filter(r => r.id !== runner.id));
  };

  useEffect(() => {
    toggleInlineRunnerRef.current = toggleInlineRunner;
  }); // Keep toggle runner ref up-to-date

  // Register CodeLens and Monaco Command
  useEffect(() => {
    if (!monacoInstance || !editorInstance) return;

    // Register command to be called when user clicks the CodeLens
    const commandId = editorInstance.addCommand(0, (ctx, funcName, lineNumber) => {
      if (toggleInlineRunnerRef.current) {
        toggleInlineRunnerRef.current(funcName, lineNumber);
      }
    });

    const lensProvider = monacoInstance.languages.registerCodeLensProvider('waluau', {
      provideCodeLenses: (model) => {
        const text = model.getValue();
        const lines = text.split('\n');
        const lenses = [];
        const funcRegex = /^\s*(?:local\s+)?function\s+([a-zA-Z_]\w*)\s*\(/;

        for (let i = 0; i < lines.length; i++) {
          const match = lines[i].match(funcRegex);
          if (match) {
            const funcName = match[1];
            const isExported = exportsListRef.current.some(exp => exp.name === funcName);
            if (isExported) {
              const lineNum = i + 1;
              lenses.push({
                range: {
                  startLineNumber: lineNum,
                  startColumn: 1,
                  endLineNumber: lineNum,
                  endColumn: 1
                },
                id: `run-${funcName}-${lineNum}`,
                command: {
                  id: commandId,
                  title: `▶ Run ${funcName}`,
                  arguments: [funcName, lineNum]
                }
              });
            }
          }
        }
        return {
          lenses,
          dispose: () => {}
        };
      },
      resolveCodeLens: (model, codeLens) => codeLens
    });

    return () => {
      lensProvider.dispose();
    };
  }, [monacoInstance, editorInstance]);

  // Clean up all view zones on unmount
  useEffect(() => {
    return () => {
      if (editorInstance && activeRunnersRef.current.length > 0) {
        editorInstance.changeViewZones((changeAccessor) => {
          for (const runner of activeRunnersRef.current) {
            if (runner.zoneId) {
              changeAccessor.removeZone(runner.zoneId);
            }
          }
        });
      }
    };
  }, [editorInstance]);

  // Load wasm-bindgen compiler module on mount.
  useEffect(() => {
    let cancelled = false;

    import('./waluau-wasm/waluau_wasm.js')
      .then(async (module) => {
        await module.default();
        if (cancelled) {
          return;
        }
        setCompileSource(() => module.compile_multi);
        setCompilerReady(true);
        setStatus('ready');
        setLoadErrorMsg('');
      })
      .catch((err) => {
        if (cancelled) {
          return;
        }
        console.error('WASM load error:', err);
        setStatus('error');
        setLoadErrorMsg(`Failed to load WASM compiler: ${err.message}`);
      });

    return () => {
      cancelled = true;
    };
  }, []);

  const compilation = useMemo(() => {
    if (!compilerReady || !compileSource) {
      return {
        output: '',
        errorMsg: status === 'error' ? loadErrorMsg : '',
      };
    }

    try {
      const parsed = compileSource(files, entryFile);
      return {
        output: parsed,
        errorMsg: '',
      };
    } catch (err) {
      const message = typeof err === 'string' ? err : err?.message || String(err);
      return {
        output: '',
        errorMsg: message,
      };
    }
  }, [files, entryFile, compileSource, compilerReady, loadErrorMsg, status]);

  const output = compilation.output;
  const errorMsg = compilation.errorMsg;
  const outputIr = typeof output === 'object' ? output.ir : '';
  const outputWat = typeof output === 'object' ? output.wat : '';
  const outputWasmBytes = typeof output === 'object' ? output.wasm : null;
  const requiresWasmGc = typeof output === 'object' ? Boolean(output.requiresWasmGc) : false;
  const displayStatus = compilerReady
    ? errorMsg
      ? 'error'
      : output
        ? 'success'
        : 'ready'
    : status;

  // Clean up all active runners when compilation or WASM outputs change
  useEffect(() => {
    if (editorInstance && activeRunnersRef.current.length > 0) {
      editorInstance.changeViewZones((changeAccessor) => {
        for (const runner of activeRunnersRef.current) {
          if (runner.zoneId) {
            changeAccessor.removeZone(runner.zoneId);
          }
        }
      });
      setTimeout(() => {
        setActiveRunners([]);
      }, 0);
    }
  }, [outputWasmBytes, editorInstance]);

  // Set compiler diagnostics markers in Monaco Editor
  useEffect(() => {
    if (!monacoInstance || !editorInstance) return;
    const models = monacoInstance.editor.getModels();
    for (const m of models) {
      monacoInstance.editor.setModelMarkers(m, 'waluau', []);
    }

    if (errorMsg) {
      const multiMatch = errorMsg.match(/^in module "([^"]+)": (.*) at (\d+)\.\.(\d+)$/);
      if (multiMatch) {
        const file = multiMatch[1];
        const cleanMessage = multiMatch[2].trim();
        const start = parseInt(multiMatch[3], 10);
        const end = parseInt(multiMatch[4], 10);

        const targetModel = models.find(m => {
          const path = m.uri.path;
          return path === file || path === '/' + file || m.uri.toString().endsWith(file);
        });

        if (targetModel) {
          const startPos = targetModel.getPositionAt(start);
          const endPos = targetModel.getPositionAt(end);
          monacoInstance.editor.setModelMarkers(targetModel, 'waluau', [
            {
              startLineNumber: startPos.lineNumber,
              startColumn: startPos.column,
              endLineNumber: endPos.lineNumber,
              endColumn: endPos.column,
              message: cleanMessage,
              severity: monacoInstance.MarkerSeverity.Error,
            },
          ]);
        }
      } else {
        const singleMatch = errorMsg.match(/(.*) at (\d+)\.\.(\d+)$/);
        const activeModel = editorInstance.getModel();
        if (activeModel) {
          if (singleMatch) {
            const cleanMessage = singleMatch[1].trim();
            const start = parseInt(singleMatch[2], 10);
            const end = parseInt(singleMatch[3], 10);
            const startPos = activeModel.getPositionAt(start);
            const endPos = activeModel.getPositionAt(end);
            monacoInstance.editor.setModelMarkers(activeModel, 'waluau', [
              {
                startLineNumber: startPos.lineNumber,
                startColumn: startPos.column,
                endLineNumber: endPos.lineNumber,
                endColumn: endPos.column,
                message: cleanMessage,
                severity: monacoInstance.MarkerSeverity.Error,
              },
            ]);
          } else {
            const entryModel = models.find(m => m.uri.path === entryFile || m.uri.toString().endsWith(entryFile)) || activeModel;
            monacoInstance.editor.setModelMarkers(entryModel, 'waluau', [
              {
                startLineNumber: 1,
                startColumn: 1,
                endLineNumber: 1,
                endColumn: entryModel.getLineLength(1) + 1,
                message: errorMsg,
                severity: monacoInstance.MarkerSeverity.Error,
              },
            ]);
          }
        }
      }
    }
  }, [errorMsg, monacoInstance, editorInstance, files, entryFile]);

  // Sync runInstance, exportsList, inputs and results when wasm changes
  useEffect(() => {
    let active = true;
    async function loadModule() {
      await Promise.resolve(); // Yield to prevent synchronous setState warnings
      if (!outputWasmBytes) {
        if (active) {
          setRunInstance(null);
          setRunError(null);
          setExportsList([]);
          setManualResults({});
          setInitLogs([]);
        }
        return;
      }
      const wasmBuffer = new Uint8Array(outputWasmBytes);
      const richSigs = output?.signatures || {};
      const list = getWasmExports(wasmBuffer)
        .filter(func => !func.name.startsWith('__waluau'))
        .map(func => {
          const richSig = (richSigs instanceof Map || (richSigs && typeof richSigs.get === 'function'))
            ? richSigs.get(func.name)
            : richSigs[func.name];
          return {
            ...func,
            richParams: richSig ? richSig.params : null,
            richReturns: richSig ? richSig.returns : null,
          };
        });
      if (active) {
        setExportsList(list);
        setFuncInputs(prev => {
          const next = { ...prev };
          let changed = false;
          for (const func of list) {
            if (!next[func.name] || next[func.name].length !== func.params.length) {
              const defaultVals = func.richParams
                ? func.richParams.map(getDefaultParamValue)
                : func.params.map(getDefaultParamValue);
              next[func.name] = defaultVals;
              changed = true;
            }
          }
          return changed ? next : prev;
        });
      }
      const capturedInitLogs = [];
      try {
        const imports = buildWaluauImports(wasmBuffer, (msg) => {
          capturedInitLogs.push(msg);
        });
        const obj = await WebAssembly.instantiate(wasmBuffer, imports, {
          builtins: ["js-string"],
          importedStringConstants: WALUAU_STRING_CONSTANTS_MODULE,
        });

        if (active) {
          setRunInstance(obj.instance);
          setRunError(null);
          setManualResults({});
          setInitLogs(capturedInitLogs);
        }
      } catch (err) {
        if (active) {
          console.error("Instantiation failed:", err);
          setRunInstance(null);
          setRunError(classifyWasmInstantiationError(err, requiresWasmGc));
          setManualResults({});
          setInitLogs(capturedInitLogs);
        }
      }
    }

    loadModule();

    return () => {
      active = false;
    };
  }, [outputWasmBytes, requiresWasmGc, output]);

  const handleInputChange = (funcName, paramIndex, value) => {
    setFuncInputs(prev => {
      const funcParams = prev[funcName] ? [...prev[funcName]] : [];
      funcParams[paramIndex] = value;
      return { ...prev, [funcName]: funcParams };
    });
  };

  const handleRecordFieldChange = (funcName, paramIndex, fieldPath, value) => {
    setFuncInputs(prev => {
      const funcParams = prev[funcName] ? [...prev[funcName]] : [];
      let current = { ...funcParams[paramIndex] };
      funcParams[paramIndex] = current;

      for (let i = 0; i < fieldPath.length - 1; i++) {
        const key = fieldPath[i];
        current[key] = { ...current[key] };
        current = current[key];
      }
      current[fieldPath[fieldPath.length - 1]] = value;

      return { ...prev, [funcName]: funcParams };
    });
  };

  const handleManualRun = (funcName, params, richParams, richReturns) => {
    const inputs = funcInputs[funcName] || (richParams || params).map(getDefaultParamValue);
    const res = executeCall(runInstance, funcName, params, richParams, richReturns, inputs);
    setManualResults(prev => ({ ...prev, [funcName]: res }));
  };

  const getResult = (funcName, params, richParams, richReturns) => {
    if (autoRun) {
      const inputs = funcInputs[funcName] || (richParams || params).map(getDefaultParamValue);
      return executeCall(runInstance, funcName, params, richParams, richReturns, inputs);
    } else {
      return manualResults[funcName] || { isIdle: true };
    }
  };

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
          <div className="panel-header">
            <h3>Source Code</h3>
            <span className="file-extension">.walu</span>
          </div>
          <div className="editor-layout">
            {/* File Explorer Sidebar */}
            <FileExplorer
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

            {activeTab === 'run' && (
              <RunTab
                status={status}
                runError={runError}
                exportsList={exportsList}
                funcInputs={funcInputs}
                initLogs={initLogs}
                autoRun={autoRun}
                setAutoRun={setAutoRun}
                handleInputChange={handleInputChange}
                handleRecordFieldChange={handleRecordFieldChange}
                handleManualRun={handleManualRun}
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
