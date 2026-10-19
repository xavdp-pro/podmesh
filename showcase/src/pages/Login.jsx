import { useState } from 'react'
import { Link, Navigate, useLocation } from 'react-router-dom'
import { motion } from 'framer-motion'
import { BookOpen, Loader2, Mail, MailCheck } from 'lucide-react'
import { api } from '../api/client'
import { useAuth } from '../auth/AuthProvider'
import { useI18n } from '../i18n/I18nProvider'
import ThemeToggle from '../components/ui/ThemeToggle'
import LanguageSwitcher from '../components/LanguageSwitcher'
import MagicLink from './MagicLink'

const TTL_MINUTES = 15
const REASON_KEYS = { 'auth.required': 'login.reason.required', 'auth.sessionExpired': 'login.reason.sessionExpired', 'auth.accessRevoked': 'login.reason.accessRevoked' }

/**
 * Magic-link request page. When the URL carries a fragment (#token) the mailed
 * link landed here: MagicLink takes over and exchanges the token for a session.
 */
export default function Login({ appName }) {
  const { user, reason } = useAuth()
  const { t, tError } = useI18n()
  const location = useLocation()
  const hasToken = Boolean(location.hash && location.hash.length > 1)
  const [email, setEmail] = useState('')
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(false)
  const [sent, setSent] = useState(false)

  if (hasToken) return <MagicLink />
  if (user) {
    const next = new URLSearchParams(location.search).get('next') || '/'
    return <Navigate to={next.startsWith('/') ? next : '/'} replace />
  }

  async function requestLink() {
    const address = email.trim()
    if (!address) return
    setError('')
    setLoading(true)
    try {
      await api.requestMagicLink(address)
      setSent(true)
    } catch (err) {
      setError(tError(err.code || err.message, 'auth.sendFailed'))
    } finally {
      setLoading(false)
    }
  }

  return (
    <div className="relative flex min-h-[100dvh] items-center justify-center bg-canvas px-4 py-10">
      <div className="absolute right-4 top-4 flex items-center gap-2">
        <LanguageSwitcher />
        <ThemeToggle />
      </div>
      <motion.div
        initial={{ opacity: 0, y: 12 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.25 }}
        className="w-full max-w-md rounded-2xl border border-line bg-surface p-7 shadow-xl"
      >
        <div className="mb-6 flex flex-col items-center text-center">
          <div className="mb-3 flex h-12 w-12 items-center justify-center rounded-2xl bg-accent text-lg font-black text-accent-fg shadow-lg">
            {appName.slice(0, 1).toUpperCase()}
          </div>
          <h1 className="text-xl font-black tracking-tight text-fg">{appName}</h1>
          <p className="mt-1 text-xs text-fg-muted">{t('login.subtitle')}</p>
        </div>

        {reason && REASON_KEYS[reason] && (
          <div className="mb-4 rounded-xl border border-warning/40 bg-warning-soft px-3.5 py-2.5 text-xs font-medium text-fg">{t(REASON_KEYS[reason])}</div>
        )}
        {error && <div className="mb-4 rounded-xl border border-danger/40 bg-danger-soft px-3.5 py-2.5 text-xs font-medium text-danger">{error}</div>}

        {sent ? (
          <div className="rounded-xl border border-success/40 bg-success-soft px-4 py-4 text-sm">
            <p className="flex items-center gap-2 font-semibold text-success">
              <MailCheck size={18} /> {t('login.sentTitle')}
            </p>
            <p className="mt-1.5 text-xs leading-relaxed text-fg-muted">{t('login.sentBody', { minutes: TTL_MINUTES })}</p>
            <button type="button" onClick={requestLink} disabled={loading} className="mt-3 text-xs font-semibold text-accent underline underline-offset-2 disabled:opacity-60">
              {t('login.resend')}
            </button>
          </div>
        ) : (
          <div className="space-y-3">
            <label htmlFor="login-email" className="block text-[11px] font-bold uppercase tracking-wider text-fg-faint">
              {t('login.email')}
            </label>
            <input
              id="login-email"
              type="email"
              autoComplete="email"
              inputMode="email"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') requestLink()
              }}
              placeholder={t('login.emailPlaceholder')}
              className="w-full rounded-xl border border-line bg-surface px-3.5 py-3 text-sm text-fg placeholder:text-fg-faint focus:border-accent focus:outline-none focus:ring-2 focus:ring-accent/20"
            />
            <button
              type="button"
              onClick={requestLink}
              disabled={loading || !email.trim()}
              className="flex w-full items-center justify-center gap-2 rounded-xl bg-accent py-3 text-sm font-semibold text-accent-fg shadow-lg transition-colors hover:bg-accent-hover disabled:opacity-60"
            >
              {loading ? <Loader2 size={17} className="animate-spin" /> : <Mail size={17} />}
              <span>{loading ? t('login.sending') : t('login.submit')}</span>
            </button>
          </div>
        )}

        {import.meta.env.DEV && <p className="mt-5 text-center text-[11px] text-fg-faint">{t('login.devHint')}</p>}

        <div className="mt-6 border-t border-line/60 pt-4 text-center">
          <Link
            to="/showcase"
            className="inline-flex items-center gap-2 rounded-xl border border-accent/30 bg-accent/5 px-3.5 py-2 text-xs font-bold text-accent transition-colors hover:bg-accent/15"
          >
            <BookOpen size={15} />
            <span>Vitrine Technique & Documentation Officielle</span>
          </Link>
        </div>
      </motion.div>
    </div>
  )
}
