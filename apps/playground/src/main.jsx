import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.jsx'
import OutputRunner from './components/OutputRunner.jsx'

const pathname = window.location.pathname;
const outputMatch = pathname.match(/^\/output\/([^/]+)/);

createRoot(document.getElementById('root')).render(
  <StrictMode>
    {outputMatch ? (
      <OutputRunner presetKey={outputMatch[1]} />
    ) : (
      <App />
    )}
  </StrictMode>,
)
