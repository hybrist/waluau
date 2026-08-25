import { useState, useEffect, useRef } from 'react';

export default function useMonacoEditor({ files, setActiveFile, entryFile, exportsList, outputWasmBytes, errorMsg, diagnostics = null, sendLspRequest = null }) {
  const [editorInstance, setEditorInstance] = useState(null);
  const [monacoInstance, setMonacoInstance] = useState(null);
  const [activeRunners, setActiveRunners] = useState([]);

  const exportsListRef = useRef(exportsList);
  const activeRunnersRef = useRef(activeRunners);
  const toggleInlineRunnerRef = useRef(null);
  const filesRef = useRef(files);

  useEffect(() => {
    exportsListRef.current = exportsList;
  }, [exportsList]);

  useEffect(() => {
    filesRef.current = files;
  }, [files]);

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
          'declare',
          'do',
          'else',
          'elseif',
          'end',
          'false',
          'function',
          'if',
          'is',
          'local',
          'not',
          'or',
          'repeat',
          'return',
          'then',
          'true',
          'type',
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

  const removeRunnerZone = (runner) => {
    if (editorInstance && runner.zoneId) {
      editorInstance.changeViewZones((changeAccessor) => {
        changeAccessor.removeZone(runner.zoneId);
      });
    }
    setActiveRunners(prev => prev.filter(r => r.id !== runner.id));
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

  // Language features (hover, go-to-definition, find-references, completion)
  // served by the worker-owned language server through asynchronous JSON-RPC
  // requests. The LSP speaks the same virtual paths the document-sync effect
  // opened, so a model's path is mapped back to its files-map key before each
  // request.
  useEffect(() => {
    if (!monacoInstance || !sendLspRequest) return;

    const filePathForResource = (resource) => {
      const resourcePath = resource.path;
      return Object.keys(filesRef.current).find(
        (path) => resourcePath === path || resourcePath === `/${path}` || resource.toString().endsWith(path)
      );
    };
    const lspPathForModel = (model) => filePathForResource(model.uri) ?? model.uri.path;
    const modelForResource = (resource) => {
      const filePath = filePathForResource(resource);
      if (!filePath) return null;
      const existing = monacoInstance.editor
        .getModels()
        .find(
          (model) =>
            model.uri.toString() === resource.toString() ||
            filePathForResource(model.uri) === filePath
        );
      return existing ?? monacoInstance.editor.createModel(
        filesRef.current[filePath],
        'waluau',
        resource,
      );
    };
    const modelForLspUri = (uri) => modelForResource(monacoInstance.Uri.parse(uri));
    const positionParams = (model, position) => ({
      textDocument: { uri: `file://${lspPathForModel(model)}` },
      position: { line: position.lineNumber - 1, character: position.column - 1 },
    });
    // The live buffer, synced before each request so position requests never
    // trail the React state round-trip.
    const liveDocument = (model) => ({ path: lspPathForModel(model), text: model.getValue() });
    const toMonacoRange = (range) => ({
      startLineNumber: range.start.line + 1,
      startColumn: range.start.character + 1,
      endLineNumber: range.end.line + 1,
      endColumn: range.end.character + 1,
    });

    const hoverProvider = monacoInstance.languages.registerHoverProvider('waluau', {
      async provideHover(model, position) {
        const result = await sendLspRequest('textDocument/hover', positionParams(model, position), liveDocument(model));
        if (!result?.contents?.value) return null;
        return {
          contents: [{ value: result.contents.value }],
          range: result.range ? toMonacoRange(result.range) : undefined,
        };
      },
    });

    const definitionProvider = monacoInstance.languages.registerDefinitionProvider('waluau', {
      async provideDefinition(model, position) {
        const result = await sendLspRequest('textDocument/definition', positionParams(model, position), liveDocument(model));
        if (!result?.uri || !result.range) return null;
        const target = modelForLspUri(result.uri);
        if (!target) return null;
        return { uri: target.uri, range: toMonacoRange(result.range) };
      },
    });

    const editorOpener = monacoInstance.editor.registerEditorOpener({
      openCodeEditor(source, resource, selectionOrPosition) {
        const filePath = filePathForResource(resource);
        const target = modelForResource(resource);
        if (!filePath || !target) return false;

        setActiveFile(filePath);
        source.setModel(target);
        if (selectionOrPosition?.endLineNumber != null) {
          source.setSelection(selectionOrPosition);
          source.revealRangeInCenter(selectionOrPosition);
        } else if (selectionOrPosition) {
          source.setPosition(selectionOrPosition);
          source.revealPositionInCenter(selectionOrPosition);
        }
        source.focus();
        return true;
      },
    });

    const referenceProvider = monacoInstance.languages.registerReferenceProvider('waluau', {
      async provideReferences(model, position, context) {
        const result = await sendLspRequest(
          'textDocument/references',
          {
            ...positionParams(model, position),
            context: { includeDeclaration: context?.includeDeclaration ?? true },
          },
          liveDocument(model)
        );
        if (!Array.isArray(result)) return [];
        return result
          .map((location) => {
            const target = modelForLspUri(location.uri);
            return target ? { uri: target.uri, range: toMonacoRange(location.range) } : null;
          })
          .filter(Boolean);
      },
    });

    // LSP CompletionItemKind -> Monaco CompletionItemKind (they use
    // different numbering).
    const kinds = monacoInstance.languages.CompletionItemKind;
    const completionKinds = {
      3: kinds.Function,
      5: kinds.Field,
      6: kinds.Variable,
      7: kinds.Class,
      9: kinds.Module,
      10: kinds.Property,
      14: kinds.Keyword,
      21: kinds.Constant,
    };
    const completionProvider = monacoInstance.languages.registerCompletionItemProvider('waluau', {
      triggerCharacters: ['.', ':'],
      async provideCompletionItems(model, position) {
        const result = await sendLspRequest('textDocument/completion', positionParams(model, position), liveDocument(model));
        if (!Array.isArray(result)) return { suggestions: [] };
        const word = model.getWordUntilPosition(position);
        const range = {
          startLineNumber: position.lineNumber,
          startColumn: word.startColumn,
          endLineNumber: position.lineNumber,
          endColumn: word.endColumn,
        };
        return {
          suggestions: result.map((item) => ({
            label: item.label,
            kind: completionKinds[item.kind] ?? kinds.Text,
            detail: item.detail ?? undefined,
            insertText: item.label,
            range,
          })),
        };
      },
    });

    return () => {
      hoverProvider.dispose();
      definitionProvider.dispose();
      referenceProvider.dispose();
      completionProvider.dispose();
      editorOpener.dispose();
    };
  }, [monacoInstance, sendLspRequest, setActiveFile]);

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

    // Language-server diagnostics (structured, multi-error, per-file) take
    // precedence; the errorMsg regex path remains for compiler stages the
    // LSP does not cover (IR lowering, codegen) and older wasm builds.
    if (diagnostics != null && diagnostics.size > 0) {
      for (const [uri, items] of diagnostics) {
        const path = uri.replace(/^file:\/\//, '');
        const targetModel = models.find(m => m.uri.path === path || m.uri.toString().endsWith(path));
        if (!targetModel) continue;
        const markers = items.map((item) => ({
          startLineNumber: item.range.start.line + 1,
          startColumn: item.range.start.character + 1,
          endLineNumber: item.range.end.line + 1,
          endColumn: item.range.end.character + 1,
          message: item.message,
          severity: item.severity === 2
            ? monacoInstance.MarkerSeverity.Warning
            : monacoInstance.MarkerSeverity.Error,
        }));
        monacoInstance.editor.setModelMarkers(targetModel, 'waluau', markers);
      }
      return;
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
  }, [errorMsg, diagnostics, monacoInstance, editorInstance, files, entryFile]);

  // Automatically dispose Monaco models if they are deleted or renamed
  useEffect(() => {
    if (!monacoInstance) return;
    const models = monacoInstance.editor.getModels();
    const filePaths = Object.keys(files);
    for (const model of models) {
      const path = model.uri.path;
      const exists = filePaths.some(f => f === path || model.uri.toString().endsWith(f));
      if (!exists) {
        model.dispose();
      }
    }
  }, [files, monacoInstance]);

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

  return {
    editorInstance,
    monacoInstance,
    activeRunners,
    handleEditorBeforeMount,
    handleEditorDidMount,
    removeRunnerZone
  };
}
