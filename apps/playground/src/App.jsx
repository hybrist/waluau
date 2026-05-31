import { useEffect, useMemo, useState, useRef } from 'react';
import { createPortal } from 'react-dom';
import Editor from '@monaco-editor/react';

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

const PRESETS = [...SINGLE_PRESETS, MULTI_PRESET].sort((left, right) =>
  left.label.localeCompare(right.label)
);

const DEFAULT_PRESET = PRESETS[0] || {
  key: 'default',
  label: 'Default',
  files: { '/main.walu': '' },
  entryFile: '/main.walu'
};

const WALUAU_IMPORT_MODULE = 'waluau';
const WALUAU_STRING_SECTION = 'waluau.str';
// Must match waluau_codegen_wasm::host::HOST_IMPORT_COUNT
const WALUAU_HOST_IMPORT_COUNT = 3;

function readU32Le(bytes, offset) {
  return (
    bytes[offset] |
    (bytes[offset + 1] << 8) |
    (bytes[offset + 2] << 16) |
    (bytes[offset + 3] << 24)
  ) >>> 0;
}

function parseWaluauStringSection(buffer) {
  const bytes = new Uint8Array(buffer);
  let pos = 8;
  while (pos < bytes.length) {
    const sectionId = bytes[pos++];
    let sectionLen = 0;
    let shift = 0;
    while (true) {
      const byte = bytes[pos++];
      sectionLen |= (byte & 0x7f) << shift;
      if ((byte & 0x80) === 0) break;
      shift += 7;
    }
    const sectionEnd = pos + sectionLen;
    if (sectionId === 0) {
      let nameLen = 0;
      shift = 0;
      while (true) {
        const byte = bytes[pos++];
        nameLen |= (byte & 0x7f) << shift;
        if ((byte & 0x80) === 0) break;
        shift += 7;
      }
      const name = new TextDecoder().decode(bytes.subarray(pos, pos + nameLen));
      pos += nameLen;
      if (name === WALUAU_STRING_SECTION) {
        const data = bytes.subarray(pos, sectionEnd);
        const count = readU32Le(data, 0);
        const strings = [];
        let offset = 4;
        for (let i = 0; i < count; i++) {
          const len = readU32Le(data, offset);
          offset += 4;
          strings.push(new TextDecoder().decode(data.subarray(offset, offset + len)));
          offset += len;
        }
        return strings;
      }
    }
    pos = sectionEnd;
  }
  return [];
}

function buildWaluauImports(strings) {
  return {
    [WALUAU_IMPORT_MODULE]: {
      string_lit: (index) => strings[index] ?? '',
      string_eq: (left, right) => (left === right ? 1 : 0),
      string_concat: (left, right) => `${left}${right}`,
    },
  };
}

// Parse WebAssembly binary to extract exports and signatures
function getWasmExports(buffer) {
  if (!buffer) return [];
  const bytes = new Uint8Array(buffer);
  let pos = 8; // Skip magic number and version
  
  // Helper to read LEB128 unsigned integer
  function readVaruint() {
    let result = 0;
    let shift = 0;
    while (true) {
      if (pos >= bytes.length) return result;
      const byte = bytes[pos++];
      result |= (byte & 0x7f) << shift;
      if ((byte & 0x80) === 0) break;
      shift += 7;
    }
    return result;
  }

  function readValTypeCode() {
    const byte = bytes[pos++];
    if (byte === 0x63 || byte === 0x64) {
      pos += 1; // heap type (e.g. extern)
      return byte === 0x63 ? 0x6f : 0x64;
    }
    return byte;
  }

  function skipTypeDefinition(form) {
    if (form === 0x60) {
      const numParams = readVaruint();
      for (let p = 0; p < numParams; p++) {
        readValTypeCode();
      }
      const numReturns = readVaruint();
      for (let r = 0; r < numReturns; r++) {
        readValTypeCode();
      }
      return;
    }
    if (form === 0x5e) {
      readValTypeCode(); // storage type
      pos += 1; // mutable flag
      return;
    }
    // Unknown composite type: bail out to end of section by caller.
    throw new Error(`unsupported wasm type form: 0x${form.toString(16)}`);
  }

  const types = [];
  const funcTypeIndices = [];
  const exports = [];
  let importFuncCount = WALUAU_HOST_IMPORT_COUNT;

  while (pos < bytes.length) {
    const sectionId = bytes[pos++];
    const sectionLength = readVaruint();
    const sectionEnd = pos + sectionLength;
    if (sectionEnd > bytes.length) break;

    if (sectionId === 2) { // Import section
      const numImports = readVaruint();
      importFuncCount = 0;
      for (let i = 0; i < numImports; i++) {
        const moduleLen = readVaruint();
        pos += moduleLen;
        const nameLen = readVaruint();
        pos += nameLen;
        const kind = bytes[pos++];
        if (kind === 0) {
          importFuncCount += 1;
          readVaruint(); // type index
        } else if (kind === 1) {
          pos += 2; // table type
        } else if (kind === 2) {
          pos += 2; // memory limits
        } else if (kind === 3) {
          pos += 2; // global type
        }
      }
    } else if (sectionId === 1) { // Type section
      const numTypes = readVaruint();
      for (let i = 0; i < numTypes; i++) {
        const form = bytes[pos++];
        if (form === 0x60) {
          const numParams = readVaruint();
          const params = [];
          for (let p = 0; p < numParams; p++) {
            params.push(readValTypeCode());
          }
          const numReturns = readVaruint();
          const returns = [];
          for (let r = 0; r < numReturns; r++) {
            returns.push(readValTypeCode());
          }
          types.push({ params, returns });
        } else {
          try {
            skipTypeDefinition(form);
          } catch {
            pos = sectionEnd;
            break;
          }
        }
      }
    } else if (sectionId === 3) { // Function section
      const numFuncs = readVaruint();
      for (let i = 0; i < numFuncs; i++) {
        funcTypeIndices.push(readVaruint());
      }
    } else if (sectionId === 7) { // Export section
      const numExports = readVaruint();
      for (let i = 0; i < numExports; i++) {
        const nameLen = readVaruint();
        const nameBytes = bytes.subarray(pos, pos + nameLen);
        pos += nameLen;
        const name = new TextDecoder().decode(nameBytes);
        const kind = bytes[pos++];
        const index = readVaruint();
        if (kind === 0) { // function export
          exports.push({ name, index });
        }
      }
    } else {
      pos = sectionEnd;
    }
  }

  return exports.map(exp => {
    const definedIndex = exp.index - importFuncCount;
    const typeIdx = funcTypeIndices[definedIndex];
    const signature = types[typeIdx] || { params: [], returns: [] };
    return {
      name: exp.name,
      params: signature.params.map(typeCode => {
        if (typeCode === 0x7f) return 'i32';
        if (typeCode === 0x7e) return 'i64';
        if (typeCode === 0x7d) return 'f32';
        if (typeCode === 0x7c) return 'f64';
        if (typeCode === 0x6f || typeCode === 0x64) return 'string';
        return 'unknown';
      }),
      returns: signature.returns.map(typeCode => {
        if (typeCode === 0x7f) return 'i32';
        if (typeCode === 0x7e) return 'i64';
        if (typeCode === 0x7d) return 'f32';
        if (typeCode === 0x7c) return 'f64';
        if (typeCode === 0x6f || typeCode === 0x64) return 'string';
        return 'unknown';
      })
    };
  });
}

function parseStringInput(valStr) {
  const trimmed = valStr.trim();
  if (
    trimmed.length >= 2 &&
    ((trimmed.startsWith('"') && trimmed.endsWith('"')) ||
     (trimmed.startsWith("'") && trimmed.endsWith("'")))
  ) {
    const inner = trimmed.slice(1, -1);
    let result = '';
    let i = 0;
    while (i < inner.length) {
      if (inner[i] === '\\' && i + 1 < inner.length) {
        const nextChar = inner[i + 1];
        if (nextChar === 'n') result += '\n';
        else if (nextChar === 't') result += '\t';
        else if (nextChar === 'r') result += '\r';
        else if (nextChar === '\\') result += '\\';
        else if (nextChar === '"') result += '"';
        else if (nextChar === "'") result += "'";
        else result += '\\' + nextChar;
        i += 2;
      } else {
        result += inner[i];
        i += 1;
      }
    }
    return result;
  }
  return valStr;
}

function getDefaultParamValue(type) {
  return type === 'string' ? '""' : '0';
}

function executeCall(instance, funcName, paramsInfo, inputValues) {
  if (!instance) return { error: 'No instance' };
  const func = instance.exports[funcName];
  if (!func) return { error: `Exported function "${funcName}" not found` };

  try {
    const parsedArgs = [];
    for (let i = 0; i < paramsInfo.length; i++) {
      const type = paramsInfo[i];
      const valStr = inputValues[i] || '0';
      
      if (type === 'i64') {
        try {
          parsedArgs.push(BigInt(valStr.trim().replace(/n$/, '')));
        } catch {
          return { error: `Parameter ${i} must be a valid 64-bit integer` };
        }
      } else if (type === 'i32') {
        const val = Number(valStr);
        if (isNaN(val) || !Number.isInteger(val)) {
          return { error: `Parameter ${i} must be a valid 32-bit integer` };
        }
        parsedArgs.push(val);
      } else if (type === 'f32' || type === 'f64') {
        const val = Number(valStr);
        if (isNaN(val)) {
          return { error: `Parameter ${i} must be a valid number` };
        }
        parsedArgs.push(val);
      } else if (type === 'string') {
        parsedArgs.push(parseStringInput(valStr));
      } else {
        parsedArgs.push(Number(valStr));
      }
    }

    const result = func(...parsedArgs);
    if (typeof result === 'bigint') {
      return { value: result.toString() + 'n' };
    } else if (typeof result === 'string') {
      return { value: JSON.stringify(result) };
    } else {
      return { value: String(result) };
    }
  } catch (err) {
    return { error: `Execution crashed: ${err.message}` };
  }
}

function classifyWasmInstantiationError(err, requiresWasmGc) {
  const message = err?.message || String(err);
  if (!requiresWasmGc) {
    return `Failed to instantiate WASM module: ${message}`;
  }
  return [
    'This module requires Wasm GC (array reference types), but this browser runtime does not support it yet.',
    `Runtime error: ${message}`
  ].join('\n');
}

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
  const [activeTab, setActiveTab] = useState('ir'); // 'ir', 'logs', 'run'
  const [runInstance, setRunInstance] = useState(null);
  const [runError, setRunError] = useState(null);
  const [exportsList, setExportsList] = useState([]);
  const [funcInputs, setFuncInputs] = useState({});
  const [autoRun, setAutoRun] = useState(true);
  const [manualResults, setManualResults] = useState({});
  const [editorInstance, setEditorInstance] = useState(null);
  const [monacoInstance, setMonacoInstance] = useState(null);
  const [activeRunners, setActiveRunners] = useState([]);

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

  const selectPreset = (preset) => {
    setFiles(preset.files);
    setActiveFile(preset.entryFile);
    setEntryFile(preset.entryFile);
  };

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
        }
        return;
      }
      const wasmBuffer = new Uint8Array(outputWasmBytes);
      const list = getWasmExports(wasmBuffer);
      if (active) {
        setExportsList(list);
        setFuncInputs(prev => {
          const next = { ...prev };
          let changed = false;
          for (const func of list) {
            if (!next[func.name] || next[func.name].length !== func.params.length) {
              next[func.name] = func.params.map(getDefaultParamValue);
              changed = true;
            }
          }
          return changed ? next : prev;
        });
      }
      try {
        const strings = parseWaluauStringSection(wasmBuffer);
        const imports = buildWaluauImports(strings);
        const obj = await WebAssembly.instantiate(wasmBuffer, imports);

        if (active) {
          setRunInstance(obj.instance);
          setRunError(null);
          setManualResults({});
        }
      } catch (err) {
        if (active) {
          console.error("Instantiation failed:", err);
          setRunInstance(null);
          setRunError(classifyWasmInstantiationError(err, requiresWasmGc));
          setManualResults({});
        }
      }
    }

    loadModule();

    return () => {
      active = false;
    };
  }, [outputWasmBytes, requiresWasmGc]);

  const handleInputChange = (funcName, paramIndex, value) => {
    setFuncInputs(prev => {
      const funcParams = prev[funcName] ? [...prev[funcName]] : [];
      funcParams[paramIndex] = value;
      return { ...prev, [funcName]: funcParams };
    });
  };

  const handleManualRun = (funcName, params) => {
    const inputs = funcInputs[funcName] || params.map(getDefaultParamValue);
    const res = executeCall(runInstance, funcName, params, inputs);
    setManualResults(prev => ({ ...prev, [funcName]: res }));
  };

  const getResult = (funcName, params) => {
    if (autoRun) {
      const inputs = funcInputs[funcName] || params.map(getDefaultParamValue);
      return executeCall(runInstance, funcName, params, inputs);
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
      <div className="presets-bar">
        <span className="presets-label">Examples:</span>
        {PRESETS.map((preset) => (
          <button key={preset.key} className="preset-btn" onClick={() => selectPreset(preset)}>
            {preset.label}
          </button>
        ))}
      </div>

      <main className="playground-main">
        {/* Editor Column */}
        <section className="column-panel editor-panel">
          <div className="panel-header">
            <h3>Source Code</h3>
            <span className="file-extension">.walu</span>
          </div>
          <div className="editor-layout">
            {/* File Explorer Sidebar */}
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
              <button
                className={`tab-btn ${activeTab === 'run' ? 'active' : ''}`}
                onClick={() => setActiveTab('run')}
              >
                Function Calling
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
              <div className="func-calling-container">
                <div className="func-calling-header">
                  <h3>Function Calling</h3>
                  <label className="autorun-toggle">
                    <input
                      type="checkbox"
                      checked={autoRun}
                      onChange={(e) => setAutoRun(e.target.checked)}
                    />
                    Auto-run on input change
                  </label>
                </div>

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
                      const inputs = funcInputs[func.name] || func.params.map(getDefaultParamValue);
                      const res = getResult(func.name, func.params);

                      return (
                        <div key={func.name} className="func-card">
                          <div className="func-signature">
                            <span className="func-signature-name">{func.name}</span>
                            <span>(</span>
                            {func.params.map((type, idx) => (
                              <span key={idx}>
                                param{idx}: <span className="func-signature-type">{type}</span>
                                {idx < func.params.length - 1 ? ', ' : ''}
                              </span>
                            ))}
                            <span>)</span>
                            <span className="func-signature-arrow"> -&gt; </span>
                            <span className="func-signature-type">
                              {func.returns.length > 0 ? func.returns.join(', ') : 'void'}
                            </span>
                          </div>

                          {func.params.length > 0 && (
                            <div className="func-inputs">
                              {func.params.map((type, idx) => (
                                <div key={idx} className="func-input-row">
                                  <label className="func-input-label">param{idx} ({type}):</label>
                                  <input
                                    type="text"
                                    className="func-input-field"
                                    value={inputs[idx] ?? '0'}
                                    onChange={(e) => handleInputChange(func.name, idx, e.target.value)}
                                    placeholder={`Enter ${type} value`}
                                  />
                                </div>
                              ))}
                            </div>
                          )}

                          {!autoRun && (
                            <button
                              className="func-run-btn"
                              onClick={() => handleManualRun(func.name, func.params)}
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
                        </div>
                      );
                    })}
                  </div>
                )}
              </div>
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
            handleManualRun={handleManualRun}
            getResult={getResult}
            autoRun={autoRun}
            onClose={() => removeRunnerZone(runner)}
          />,
          runner.domNode
        )
      )}
    </div>
  );
}

function InlineRunner({
  funcName,
  exportsList,
  funcInputs,
  handleInputChange,
  handleManualRun,
  getResult,
  autoRun,
  onClose,
}) {
  const func = exportsList.find((e) => e.name === funcName);
  if (!func) return null;

  const inputs = funcInputs[funcName] || func.params.map(getDefaultParamValue);
  const res = getResult(funcName, func.params);

  return (
    <div className="inline-runner-widget">
      <div className="inline-runner-header">
        <div className="inline-runner-signature">
          <span className="inline-runner-name">{func.name}</span>
          <span>(</span>
          {func.params.map((type, idx) => (
            <span key={idx}>
              param{idx}: <span className="inline-runner-type">{type}</span>
              {idx < func.params.length - 1 ? ', ' : ''}
            </span>
          ))}
          <span>)</span>
          <span className="inline-runner-arrow"> -&gt; </span>
          <span className="inline-runner-type">
            {func.returns.length > 0 ? func.returns.join(', ') : 'void'}
          </span>
        </div>
        <button className="inline-runner-close" onClick={onClose} title="Close inline runner">
          &times;
        </button>
      </div>

      {func.params.length > 0 && (
        <div className="inline-runner-inputs">
          {func.params.map((type, idx) => (
            <div key={idx} className="inline-runner-input-row">
              <label className="inline-runner-label">param{idx} ({type}):</label>
              <input
                type="text"
                className="inline-runner-field"
                value={inputs[idx] ?? '0'}
                onChange={(e) => handleInputChange(func.name, idx, e.target.value)}
                placeholder={`Enter ${type} value`}
              />
            </div>
          ))}
        </div>
      )}

      <div className="inline-runner-actions">
        {!autoRun && (
          <button
            className="inline-runner-run-btn"
            onClick={() => handleManualRun(func.name, func.params)}
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
