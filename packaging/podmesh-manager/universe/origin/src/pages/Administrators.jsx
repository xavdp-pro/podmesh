import { useState } from 'react'
import { Plus, UserX } from 'lucide-react'
import toast from 'react-hot-toast'
import { useI18n } from '../store/useLocaleStore'
import { useSession } from '../store/useSessionStore'
import { createAdministrator, revokeAdministrator } from '../api/admin'
import SlideOver from '../components/SlideOver'
import ConfirmModal from '../components/ConfirmModal'
import PasswordField from '../components/PasswordField'
import TextField from '../components/TextField'
import Notice from '../components/Notice'

export default function Administrators() {
  const { t } = useI18n()
  const state = useSession(s => s.state)
  const refresh = useSession(s => s.refresh)
  const [creating, setCreating] = useState(false)
  const [revoking, setRevoking] = useState(null)
  const [login, setLogin] = useState('')
  const [password, setPassword] = useState('')
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const rows = state?.administrators || []

  async function create(e) {
    e.preventDefault()
    if (busy) return
    setBusy(true); setError('')
    try {
      await createAdministrator(login, password)
      toast.success(t('admins.created', { login: login.trim().toLowerCase() }))
      setLogin(''); setPassword(''); setCreating(false)
      await refresh()
    } catch (err) {
      setError(err?.error || t('common.refused'))
    } finally {
      setBusy(false)
    }
  }
  async function revoke() {
    if (busy || !revoking) return
    setBusy(true)
    try {
      await revokeAdministrator(revoking)
      toast.success(t('admins.revoked', { login: revoking }))
      setRevoking(null)
      await refresh()
    } catch (err) {
      toast.error(err?.error || t('common.refused'))
    } finally {
      setBusy(false)
    }
  }
  return (
    <section className="max-w-3xl">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h1 className="text-xl font-semibold text-ink-900">{t('admins.title')}</h1>
          <p className="mt-1 text-sm text-ink-700">{t('admins.lead')}</p>
        </div>
        <button type="button" onClick={() => { setError(''); setCreating(true) }} className="flex shrink-0 items-center gap-1.5 rounded-md bg-green-600 px-3 py-2 text-sm font-medium text-cream-50 hover:bg-green-700"><Plus size={15} />{t('admins.create')}</button>
      </div>
      <table className="mt-6 w-full text-sm">
        <thead><tr className="border-b border-cream-300 text-left text-xs uppercase tracking-wide text-ink-500"><th className="py-2">{t('admins.login')}</th><th className="py-2">{t('admins.scope')}</th><th /></tr></thead>
        <tbody>
          {rows.map(r => (
            <tr key={r.login} className="border-b border-cream-300">
              <td className="py-2.5"><code>{r.login}</code>{r.login === state.login && <span className="ml-2 text-xs text-ink-500">({t('admins.you')})</span>}</td>
              <td className="py-2.5 text-ink-700">{r.scopes.join(', ')}{r.conflict && <em className="ml-2 text-terra-600">— {t('admins.conflict')}</em>}</td>
              <td className="py-2.5 text-right">
                {r.login !== state.login && rows.length > 1 && (
                  <button type="button" onClick={() => setRevoking(r.login)} aria-label={`${t('admins.revoke')} ${r.login}`} className="inline-flex items-center gap-1 rounded-md px-2 py-1 text-xs text-ink-500 hover:bg-terra-100 hover:text-terra-600"><UserX size={14} />{t('admins.revoke')}</button>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      <SlideOver open={creating} onClose={() => !busy && setCreating(false)} title={t('admins.create')}>
        <p className="text-sm text-ink-700">{t('admins.createLead', { scope: state?.scope })}</p>
        <form onSubmit={create} noValidate className="mt-5 space-y-4">
          <Notice text={error} bad />
          <TextField label={t('admins.newLogin')} value={login} onChange={setLogin} autoFocus />
          <PasswordField label={t('admins.newPassword')} value={password} onChange={setPassword} autoComplete="new-password" hint={t('admins.newHint')} />
          <button type="submit" disabled={busy} className="w-full rounded-md bg-green-600 py-2.5 text-sm font-medium text-cream-50 hover:bg-green-700 disabled:opacity-60">{busy ? t('admins.creating') : t('admins.create')}</button>
        </form>
      </SlideOver>
      <ConfirmModal open={!!revoking} title={t('admins.revokeTitle', { login: revoking })} message={t('admins.revokeMessage')} confirmLabel={t('admins.revokeConfirm')} pending={busy} onConfirm={revoke} onCancel={() => !busy && setRevoking(null)} />
    </section>
  )
}
