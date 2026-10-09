import { useState } from 'react'
import { useI18n } from '../store/useLocaleStore'
import { useSession } from '../store/useSessionStore'
import { login as signIn } from '../api/admin'
import PasswordField from '../components/PasswordField'
import TextField from '../components/TextField'
import Notice from '../components/Notice'
import LanguageSwitcher from '../components/LanguageSwitcher'

export default function Login() {
  const { t } = useI18n()
  const refresh = useSession(s => s.refresh)
  const flash = useSession(s => s.flash)
  const setFlash = useSession(s => s.setFlash)
  const [login, setLogin] = useState('')
  const [password, setPassword] = useState('')
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)

  // The form is a container: a password manager pairs the login with its password and Enter
  // submits; the submission is intercepted and becomes one JSON call. It has no method and no
  // action, and the page's policy forbids a native submission anyway.
  async function submit(e) {
    e.preventDefault()
    if (busy) return
    setBusy(true); setError(''); setFlash('')
    try {
      await signIn(login, password)
      await refresh()
    } catch (err) {
      setError(err?.error || t('common.refused'))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="flex min-h-dvh items-center justify-center px-4 py-10">
      <div className="absolute right-4 top-4"><LanguageSwitcher /></div>
      <div className="w-full max-w-sm rounded-xl border border-cream-300 bg-cream-50 p-8 shadow-sm">
        <p className="text-xs font-semibold tracking-widest text-ink-500">{t('brand')}</p>
        <h1 className="mt-1 text-xl font-semibold text-ink-900">{t('login.title')}</h1>
        <form onSubmit={submit} noValidate className="mt-6 space-y-4">
          <Notice text={error || (flash ? t(`login.${flash}`) : '')} bad />
          <TextField label={t('login.administrator')} value={login} onChange={setLogin} autoComplete="username" autoFocus />
          <PasswordField label={t('login.password')} value={password} onChange={setPassword} autoComplete="current-password" />
          <button type="submit" disabled={busy} className="w-full rounded-md bg-green-600 py-2.5 text-sm font-medium text-cream-50 transition hover:bg-green-700 disabled:opacity-60">{busy ? t('login.submitting') : t('login.submit')}</button>
        </form>
      </div>
    </div>
  )
}
