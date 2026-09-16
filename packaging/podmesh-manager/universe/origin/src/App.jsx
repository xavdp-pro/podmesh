import { useEffect } from 'react'
import { BrowserRouter, Routes, Route, Navigate } from 'react-router-dom'
import { useI18n } from './store/useLocaleStore'
import { useSession } from './store/useSessionStore'
import { setExpiredHandler } from './api'
import Layout from './components/Layout'
import Login from './pages/Login'
import ChangePassword from './pages/ChangePassword'
import Administrators from './pages/Administrators'
import Status from './pages/Status'
import Closed from './pages/Closed'

// The manager decides what this visitor sees; the page draws it. One state call, then a view.
export default function App() {
  const { t } = useI18n()
  const { state, loading, failure, refresh, expired } = useSession()
  useEffect(() => { setExpiredHandler(expired); refresh() }, [refresh, expired])
  if (failure === 'not the governor') return <Closed kind="closed" />
  if (failure) return <main className="booting" role="alert"><p>{t('brand')}</p><p>{t('common.unreachable')}</p></main>
  if (loading && !state) return <main className="booting"><p>{t('brand')}</p><p>{t('common.loading')}</p></main>
  if (state.view === 'none') return <Closed kind="none" />
  if (state.view === 'login') return <Login />
  if (state.view === 'change') return <ChangePassword />
  return (
    <BrowserRouter basename="/admin">
      <Routes>
        <Route element={<Layout />}>
          <Route index element={<Administrators />} />
          <Route path="status" element={<Status />} />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Route>
      </Routes>
    </BrowserRouter>
  )
}
