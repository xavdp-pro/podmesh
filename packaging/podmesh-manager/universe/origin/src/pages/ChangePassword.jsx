import { useState } from 'react'
import toast from 'react-hot-toast'
import { useI18n } from '../store/useLocaleStore'
import { useSession } from '../store/useSessionStore'
import { changePassword } from '../api/admin'
import PasswordField from '../components/PasswordField'
import Notice from '../components/Notice'
import LanguageSwitcher from '../components/LanguageSwitcher'

export default function ChangePassword() {
  const { t } = useI18n()
  const state = useSession(s => s.state)
  const refresh = useSession(s => s.refresh)
  const [current, setCurrent] = useState('')
  const [next, setNext] = useState('')
  const [again, setAgain] = useState('')
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  async function submit(e) {
    e.preventDefault()
    if (busy) return
    setBusy(true); setError('')
    try {
      const r = await changePassword(current, next, again)
      toast.success(t('change.done'))
      await refresh()
      return r
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
        <h1 className="mt-1 text-xl font-semibold text-ink-900">{t('change.title')}</h1>
        <p className="mt-3 rounded-md border-l-2 border-green-600 bg-cream-200 px-3 py-2 text-sm text-ink-700">{t('change.why')}</p>
        <form onSubmit={submit} noValidate className="mt-5 space-y-4">
          <input type="text" name="username" autoComplete="username" value={state?.login || ''} readOnly hidden aria-hidden="true" />
          <Notice text={error} bad />
          <PasswordField label={t('change.current')} value={current} onChange={setCurrent} autoComplete="current-password" autoFocus />
          <PasswordField label={t('change.next')} value={next} onChange={setNext} autoComplete="new-password" hint={t('admins.newHint')} />
          <PasswordField label={t('change.again')} value={again} onChange={setAgain} autoComplete="new-password" />
          <button type="submit" disabled={busy} className="w-full rounded-md bg-green-600 py-2.5 text-sm font-medium text-cream-50 transition hover:bg-green-700 disabled:opacity-60">{busy ? t('change.submitting') : t('change.submit')}</button>
        </form>
        <p className="mt-4 text-xs text-ink-500">{t('nav.signedInAs')} <code>{state?.login}</code></p>
      </div>
    </div>
  )
}
