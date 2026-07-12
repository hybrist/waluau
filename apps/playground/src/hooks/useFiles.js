import { useState, useMemo, useCallback } from 'react';
import {
  fixtureModules,
  moduleFixtures,
  pokerTricksFixtures,
  snakeFixtures,
  gameEngineFixtures,
  conformanceModules,
  filesForConformancePreset,
  MULTI_PRESET,
  KANBAN_PRESET,
  POKER_TRICKS_PRESET,
  SNAKE_PRESET,
  GAME_ENGINE_PRESET,
  DEFAULT_PRESET,
  PRESETS
} from '../utils/presets.js';

const EXAMPLE_QUERY_PARAM = 'example';

function presetFromUrl() {
  const key = new URLSearchParams(window.location.search).get(EXAMPLE_QUERY_PARAM);
  if (!key) return null;
  return PRESETS.find((preset) => preset.key === key) ?? null;
}

function setExampleQueryParam(key) {
  const url = new URL(window.location.href);
  url.searchParams.set(EXAMPLE_QUERY_PARAM, key);
  window.history.replaceState(null, '', url);
}

export default function useFiles() {
  const initialPreset = useMemo(() => presetFromUrl() ?? DEFAULT_PRESET, []);
  const [files, setFiles] = useState(initialPreset.files);
  const [activeFile, setActiveFile] = useState(initialPreset.entryFile);
  const [entryFile, setEntryFile] = useState(initialPreset.entryFile);
  const [editingFile, setEditingFile] = useState(null);
  const [editingValue, setEditingValue] = useState('');

  const selectPreset = useCallback((preset) => {
    setFiles(preset.files);
    setActiveFile(preset.entryFile);
    setEntryFile(preset.entryFile);
    setExampleQueryParam(preset.key);
  }, []);

  const handleAddFile = useCallback((name) => {
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
  }, [files]);

  const handleDeleteFile = useCallback((filename) => {
    const fileKeys = Object.keys(files);
    if (fileKeys.length <= 1) {
      alert('Cannot delete the last remaining file');
      return;
    }
    if (confirm(`Are you sure you want to delete ${filename}?`)) {
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
  }, [files, activeFile, entryFile]);

  const handleRenameFile = useCallback((oldName, newName) => {
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
  }, [files, activeFile, entryFile]);

  const handleFileChange = useCallback((filename, value) => {
    setFiles(prev => ({
      ...prev,
      [filename]: value
    }));
  }, []);

  const handleSetEntryFile = useCallback((filename) => {
    setEntryFile(filename);
  }, []);

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
            files: filesForConformancePreset(filename, source),
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
          selectPreset(MULTI_PRESET);
          setActiveFile(`/${filename}`);
        }
      });
    }

    items.push({
      type: 'fixture',
      path: 'fixtures/kanban/app.walu',
      name: 'kanban/app.walu',
      source: KANBAN_PRESET.files[KANBAN_PRESET.entryFile],
      category: 'Fixture',
      onSelect: () => {
        selectPreset(KANBAN_PRESET);
      }
    });

    // 4. Snake fixture files
    for (const [path, source] of Object.entries(snakeFixtures)) {
      const filename = path.split('/').pop();
      items.push({
        type: 'fixture',
        path,
        name: `snake/${filename}`,
        source,
        category: 'Fixture',
        onSelect: () => {
          selectPreset(SNAKE_PRESET);
          setActiveFile(`/fixtures/snake/${filename}`);
        }
      });
    }

    // 5. Arcane Heist fixture files
    for (const [path, source] of Object.entries(pokerTricksFixtures)) {
      const filename = path.split('/').pop();
      items.push({
        type: 'fixture',
        path,
        name: `poker-tricks/${filename}`,
        source,
        category: 'Fixture',
        onSelect: () => {
          selectPreset(POKER_TRICKS_PRESET);
          setActiveFile(`/fixtures/poker-tricks/${filename}`);
        }
      });
    }

    // 6. 2D game engine fixture files
    for (const [path, source] of Object.entries(gameEngineFixtures)) {
      const filename = path.split('/').pop();
      items.push({
        type: 'fixture',
        path,
        name: `game-engine/${filename}`,
        source,
        category: 'Fixture',
        onSelect: () => {
          selectPreset(GAME_ENGINE_PRESET);
          setActiveFile(`/fixtures/game-engine/${filename}`);
        }
      });
    }

    return items.sort((left, right) => left.name.localeCompare(right.name));
  }, [selectPreset]);

  return {
    files,
    setFiles,
    activeFile,
    setActiveFile,
    entryFile,
    setEntryFile,
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
  };
}
