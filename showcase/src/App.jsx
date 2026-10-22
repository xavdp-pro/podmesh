import React from 'react'
import { BrowserRouter, Route, Routes } from 'react-router-dom'
import { I18nProvider } from './i18n/I18nProvider'
import TechnicalShowcasePage from './pages/TechnicalShowcasePage'

export default function App() {
  return (
    <I18nProvider>
      <BrowserRouter>
        <Routes>
          <Route path="/" element={<TechnicalShowcasePage />} />
          <Route path="/showcase" element={<TechnicalShowcasePage />} />
          <Route path="/technical-showcase" element={<TechnicalShowcasePage />} />
          <Route path="/validation" element={<TechnicalShowcasePage />} />
          <Route path="*" element={<TechnicalShowcasePage />} />
        </Routes>
      </BrowserRouter>
    </I18nProvider>
  )
}
