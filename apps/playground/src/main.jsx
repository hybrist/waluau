/* eslint-disable react-refresh/only-export-components */
import { StrictMode, lazy, Suspense } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'

const App = lazy(() => import('./App.jsx'));
const OutputRunner = lazy(() => import('./components/OutputRunner.jsx'));

const pathname = window.location.pathname;
const outputMatch = pathname.match(/^\/output\/([^/]+)/);

createRoot(document.getElementById('root')).render(
  <StrictMode>
    <Suspense fallback={<div className="output-runner-loading"><div className="spinner"></div></div>}>
      {outputMatch ? (
        <OutputRunner presetKey={outputMatch[1]} />
      ) : (
        <App />
      )}
    </Suspense>
  </StrictMode>,
)
