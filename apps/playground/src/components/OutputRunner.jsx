import { useEffect } from 'react';
import useWaluauCompiler from '../hooks/useWaluauCompiler.js';
import { PRESETS } from '../utils/presets.js';
import { cleanupDomOutput } from '../utils/wasm.js';

export default function OutputRunner({ presetKey }) {
  const preset = PRESETS.find((p) => p.key === presetKey);

  if (!preset) {
    return (
      <div className="output-runner-error">
        <h2>Preset Not Found</h2>
        <p>Could not find a preset with key "{presetKey}".</p>
      </div>
    );
  }

  return <PresetRunner preset={preset} />;
}

function PresetRunner({ preset }) {
  const {
    displayStatus,
    errorMsg,
    runError,
    setDomOutputRoot,
  } = useWaluauCompiler({
    files: preset.files,
    entryFile: preset.entryFile,
  });

  // Mount DOM output to the actual window document
  useEffect(() => {
    setDomOutputRoot(document);
    return () => {
      setDomOutputRoot(null);
      cleanupDomOutput(document);
      const gameRoot = document.getElementById('walua-game');
      if (gameRoot) {
        gameRoot.remove();
      }
      document.body.removeAttribute('style');
    };
  }, [setDomOutputRoot]);

  // Hide the React #root when compiled successfully and the game starts running
  useEffect(() => {
    const root = document.getElementById('root');
    if (!root) return;

    if (displayStatus === 'success' && !runError) {
      root.style.display = 'none';
    } else {
      root.style.display = '';
    }

    return () => {
      root.style.display = '';
    };
  }, [displayStatus, runError]);

  if (displayStatus === 'loading' || displayStatus === 'ready') {
    return (
      <div className="output-runner-loading">
        <div className="spinner"></div>
        <p>Loading compiler & compiling {preset.label}...</p>
      </div>
    );
  }

  if (displayStatus === 'error') {
    return (
      <div className="output-runner-error">
        <h2>Compilation Failed</h2>
        <pre className="diagnostic-output">{errorMsg}</pre>
      </div>
    );
  }

  if (runError) {
    return (
      <div className="output-runner-error">
        <h2>Runtime Execution Error</h2>
        <pre className="diagnostic-output">{runError}</pre>
      </div>
    );
  }

  // Once loaded and running successfully, the game renders directly to document.body,
  // and `#root` is hidden. So we render nothing visible in React.
  return null;
}
