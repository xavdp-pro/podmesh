import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter } from 'react-router-dom'
import App from './App'
import Boundary from './components/Boundary'
import './index.css'

createRoot(document.getElementById('root')).render(
  <StrictMode>
    <Boundary>
      <BrowserRouter basename="/next">
        <App />
      </BrowserRouter>
    </Boundary>
  </StrictMode>,
)
