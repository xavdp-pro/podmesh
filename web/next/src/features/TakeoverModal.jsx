import { useState } from 'react'
import toast from 'react-hot-toast'
import ConfirmModal from '../components/ConfirmModal'
import TextField from '../components/TextField'
import Segmented from '../components/Segmented'
import Notice from '../components/Notice'
import { useI18n } from '../store/useLocaleStore'
import { postReplication } from '../api/console'
import { takeoverBody, ago, duration } from '../model/replication'

// Taking a universe over on a standby that holds a copy of it. The modal states what the copy is --
// its age, so what would be lost; whether it comes back with its memory (a live capture) or
// restarted afresh (a stopped one) -- and asks which situation this is: a switchover the operator
// chose while the active host is fine, or an active host that is lost. The answer, or the refusal,
// stays in the modal where the operator looks; a toast echoes a success.
export default function TakeoverModal({ open, row, standby, copy, onClose, onDone }) {
  const { t } = useI18n()
  const [planned, setPlanned] = useState(true)
  // Without a present copy, only a planned switchover can proceed: it takes a fresh copy first.
  const hasCopy = !!copy?.present_on_host
  const [auth, setAuth] = useState('')
  const [pending, setPending] = useState(false)
  const [error, setError] = useState('')
  const [result, setResult] = useState(null)
  const live = copy?.capture === 'live'

  async function proceed() {
    if (!row || !standby) return
    setPending(true); setError('')
    try {
      if (!planned && !hasCopy) throw Error('takeover.noCopy')
      const body = takeoverBody({ host: row.host.id, universe_uuid: row.uuid, authorization_ref: auth, standby: standby.id, planned })
      const report = await postReplication(body)
      if (report?.result !== 'taken_over') throw { error: report?.error || t('common.refused') }
      setResult(report)
      toast.success(t('takeover.toast', { name: row.name, host: standby.name }))
      onDone?.({ ...report, toName: standby.name })
    } catch (e) {
      const code = e instanceof Error ? e.message : null
      setError(code ? t(`validation.${code}`) : e?.error || e?.message || t('common.refused'))
    } finally { setPending(false) }
  }

  return (
    <ConfirmModal open={open} danger pending={pending} onCancel={onClose} onConfirm={proceed} hideConfirm={!!result}
      title={t('takeover.title', { host: standby?.name || '' })}
      message={row ? t('takeover.lead', { name: row.name, from: row.host.name, to: standby?.name || '' }) : ''}
      confirmLabel={t('takeover.confirm')}>
      {!hasCopy && !result && <p className="rounded-md border border-amber-600/30 bg-amber-100 px-3 py-2 text-sm text-amber-600">{t('takeover.noCopy')}</p>}
      {hasCopy && !result && (
        <ul className="space-y-1 rounded-md border border-cream-300 bg-cream-100 px-3 py-2 text-sm text-ink-700">
          <li><span className="font-medium text-ink-900">{t('takeover.age', { age: ago(copy.age_seconds, t) })}</span> {t('takeover.ageLost')}</li>
          <li>{live ? t('takeover.withMemory') : t('takeover.afresh')}</li>
          <li className="text-xs text-ink-500">{t('replication.generation', { n: copy.generation })} · {live ? t('replication.live') : t('replication.stopped')}</li>
        </ul>
      )}
      {!result && (
        <>
          <Segmented label={t('takeover.situation')} value={planned ? 'planned' : 'lost'} onChange={v => setPlanned(v === 'planned')} disabled={pending} options={[
            { value: 'planned', label: t('takeover.planned'), hint: t('takeover.plannedHint') },
            { value: 'lost', label: t('takeover.lost'), hint: hasCopy ? t('takeover.lostHint') : t('takeover.lostNeedsCopy'), disabled: !hasCopy },
          ]} />
          <TextField label={t('common.authorization')} value={auth} onChange={setAuth} placeholder={t('common.authorizationPlaceholder')} />
        </>
      )}
      <Notice bad text={error} />
      {result && (
        <Notice>
          <p className="font-medium">{t('takeover.done', { to: standby?.name || result.to })}</p>
          <p className="mt-1 text-xs">
            {result.capture === 'live' ? t('takeover.doneMemory') : t('takeover.doneAfresh')} · {t('replication.generation', { n: result.generation })} · {t('takeover.copyAge', { age: duration(result.copy_age_seconds, t) })}
            {result.waited_seconds ? ` · ${t('takeover.waited', { s: duration(result.waited_seconds, t) })}` : ''} · {t('takeover.promotion', { s: duration(result.promotion_seconds, t) })}
          </p>
        </Notice>
      )}
    </ConfirmModal>
  )
}
