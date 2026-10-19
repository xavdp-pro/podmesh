import { useEffect, useRef, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { Loader2, ShieldAlert } from 'lucide-react'
import { api } from '../api/client'
import { useAuth } from '../auth/AuthProvider'
import { useI18n } from '../i18n/I18nProvider'

/**
 * Consumes the e-mailed link. The token sits in the URL fragment, which no
 * server or mail scanner receives; it is removed from the address bar first,
 * then exchanged with a POST for the session cookie.
 */
export default function MagicLink() {
  const navigate = useNavigate()
  const { refresh } = useAuth()
  const { t, tError } = useI18n()
  const [error, setError] = useState('')
  const started = useRef(false)

  useEffect(() => {
    if (started.current) return
    started.current = true
    const token = window.location.hash.replace(/^#/, '').trim()
    window.history.replaceState(null, '', window.location.pathname + window.location.search)
    if (!token) {
      setError(t('magic.missing'))
      return
    }
    api.verifyMagicLink(token)
      .then(async () => {
        await refresh()
        navigate('/', { replace: true })
      })
      .catch((err) => setError(tError(err.code || err.message, 'auth.linkInvalid')))
  }, [navigate, refresh, t, tError])

  return (
    <div className="flex min-h-[100dvh] items-center justify-center bg-canvas px-4">
      <div className="w-full max-w-sm rounded-2xl border border-line bg-surface p-7 text-center shadow-xl">
        {error ? (
          <>
            <p className="flex items-center justify-center gap-2 text-sm font-semibold text-danger">
              <ShieldAlert size={18} /> {error}
            </p>
            <button type="button" onClick={() => navigate('/login', { replace: true })} className="mt-5 w-full rounded-xl bg-accent py-2.5 text-sm font-semibold text-accent-fg hover:bg-accent-hover">
              {t('magic.requestNew')}
            </button>
          </>
        ) : (
          <p className="flex items-center justify-center gap-2 text-sm text-fg-muted">
            <Loader2 size={18} className="animate-spin" /> {t('magic.connecting')}
          </p>
        )}
      </div>
    </div>
  )
}
