import { useEffect } from 'react'
import { Routes, Route, Navigate } from 'react-router-dom'
import { Toaster } from 'react-hot-toast'
import Layout from './components/Layout'
import Universes from './pages/Universes'
import Health from './pages/Health'
import Run from './pages/Run'
import { useSession } from './store/useSessionStore'

export default function App() {
  useEffect(() => { useSession.getState().refresh() }, [])
  return (
    <>
      <Toaster position="top-right" toastOptions={{ style: { background: '#fbf8f1', color: '#1c1916', border: '1px solid #e4dccb' } }} />
      <Routes>
        <Route element={<Layout />}>
          <Route index element={<Universes />} />
          <Route path="health" element={<Health />} />
          <Route path="run" element={<Run />} />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Route>
      </Routes>
    </>
  )
}
