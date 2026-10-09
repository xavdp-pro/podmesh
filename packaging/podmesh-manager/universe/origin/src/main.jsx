import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { Toaster } from 'react-hot-toast'
import './index.css'
import App from './App.jsx'
import Boundary from './components/Boundary.jsx'

createRoot(document.getElementById('root')).render(
  <StrictMode>
    <Boundary>
      <App />
      <Toaster position="bottom-right" toastOptions={{ duration: 3500, style: { fontSize: '14px', background: '#fbf8f1', color: '#1c1916', border: '1px solid #d8cfbe' }, success: { iconTheme: { primary: '#215547', secondary: '#fbf8f1' } }, error: { iconTheme: { primary: '#8c3b2e', secondary: '#fbf8f1' } } }} />
    </Boundary>
  </StrictMode>,
)
