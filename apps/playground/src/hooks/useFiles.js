import { useState, useMemo, useCallback } from 'react';
import {
  fixtureModules,
  moduleFixtures,
  conformanceModules,
  MULTI_PRESET,
  DEFAULT_PRESET
} from '../utils/presets.js';

export default function useFiles() {
  const [files, setFiles] = useState(DEFAULT_PRESET.files);
  const [activeFile, setActiveFile] = useState(DEFAULT_PRESET.entryFile);
  const [entryFile, setEntryFile] = useState(DEFAULT_PRESET.entryFile);
  const [editingFile, setEditingFile] = useState(null);
  const [editingValue, setEditingValue] = useState('');

  const selectPreset = useCallback((preset) => {
    setFiles(preset.files);
    setActiveFile(preset.entryFile);
    setEntryFile(preset.entryFile);
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
