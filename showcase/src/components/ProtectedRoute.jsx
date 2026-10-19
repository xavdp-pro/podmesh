import { Navigate, Outlet, useLocation } from 'react-router-dom'
import { useAuth } from '../auth/AuthProvider'
import { useI18n } from '../i18n/I18nProvider'

/** Gate for signed-in routes. Guests go to /login with the page they wanted. */
export default function ProtectedRoute() {
  const { user } = useAuth()
  const { t } = useI18n()
  const location = useLocation()

  if (user === undefined) {
    return (
      <div className="flex h-[100dvh] items-center justify-center bg-canvas text-sm text-fg-muted">{t('app.loading')}</div>
    )
  }
  if (!user) {
    const next = encodeURIComponent(location.pathname + location.search)
    return <Navigate to={`/login?next=${next}`} replace />
  }
  return <Outlet />
}
