import { useState } from 'react'
import { ShieldCheck, ShieldOff } from 'lucide-react'
import toast from 'react-hot-toast'
import TextField from '../components/TextField'
import Checkbox from '../components/Checkbox'
import Notice from '../components/Notice'
import Badge from '../components/Badge'
import Button from '../components/Button'
import ConfirmModal from '../components/ConfirmModal'
import { useI18n } from '../store/useLocaleStore'
import { postReplication } from '../api/console'
import { guardBody, unguardBody, ago } from '../model/replication'

// Continuity of service: the guardian on the workstation renews the universe's lease every tick and, once the
// active host has not been renewed for lease + margin, takes the universe over on the first standby with a copy.
// The hosts' own self-fence is what stops the universe on a host cut from the workstation; the panel shows
// whether each host has it, because without it a failover makes a second instance.
export default function ContinuityPanel({ row, status, onChanged }) {
  const { t } = useI18n()
  const guard = status?.guard
  const guarded = !!guard && guard.state !== 'disarmed' && guard.armed
  const [lease, setLease] = useState(String(guard?.lease_seconds || 30))
  const [margin, setMargin] = useState(String(guard?.takeover_margin_seconds || 15))
  const [tick, setTick] = useState(String(guard?.tick_seconds || 10))
  const [keepStale, setKeepStale] = useState(!!guard?.keep_stale)
  const [auth, setAuth] = useState('')
  const [busy, setBusy] = useState('')
  const [error, setError] = useState('')
  const [confirmOff, setConfirmOff] = useState(false)
  const fences = status?.fence || []
  const unfenced = fences.filter(f => !(f.mandate_present && f.timer_active))
  const canAct = row.host.allowActions
  const rto = Number(lease) + Number(margin) + Number(tick) + 2

  async function act(action) {
    setBusy(action); setError('')
    try {
      const body = action === 'guard'
        ? guardBody({ host: row.host.id, universe_uuid: row.uuid, authorization_ref: auth, lease_seconds: lease, takeover_margin_seconds: margin, tick_seconds: tick, keep_stale: keepStale })
        : unguardBody({ host: row.host.id, universe_uuid: row.uuid, authorization_ref: auth })
      const report = await postReplication(body)
      if (report?.result !== (action === 'guard' ? 'guarded' : 'unguarded')) throw { error: report?.error || t('common.refused') }
      toast.success(t(`continuity.toast.${action}`))
      setConfirmOff(false)
      onChanged?.()
    } catch (e) {
      const code = e instanceof Error ? e.message : null
      setError(code ? t(`validation.${code}`) : e?.error || e?.message || t('common.refused'))
    } finally { setBusy('') }
  }

  return (
    <section className="space-y-3 rounded-lg border border-cream-300 bg-cream-100/60 p-3" aria-labelledby="continuity-title">
      <div className="flex flex-wrap items-center gap-2">
        <h3 id="continuity-title" className="flex items-center gap-1.5 text-sm font-semibold text-ink-900">{guarded ? <ShieldCheck size={15} className="text-green-600" /> : <ShieldOff size={15} className="text-ink-500" />}{t('continuity.title')}</h3>
        <Badge tone={guarded ? 'green' : 'muted'}>{guarded ? t('continuity.guarded') : t('continuity.unguarded')}</Badge>
        {guard?.state === 'failing_over' && <Badge tone="red">{t('continuity.failingOver')}</Badge>}
      </div>
      <p className="text-sm text-ink-700">{t('continuity.lead')}</p>
      {guarded && (
        <dl className="grid grid-cols-2 gap-2 text-sm sm:grid-cols-4">
          <div className="rounded-md border border-cream-300 bg-cream-50 px-3 py-2"><dt className="text-xs text-ink-500">{t('continuity.lastTick')}</dt><dd>{guard.last_tick_age_seconds == null ? '—' : ago(guard.last_tick_age_seconds, t)}</dd></div>
          <div className="rounded-md border border-cream-300 bg-cream-50 px-3 py-2"><dt className="text-xs text-ink-500">{t('continuity.lastRenewal')}</dt><dd>{guard.last_renewal_age_seconds == null ? '—' : ago(guard.last_renewal_age_seconds, t)}</dd></div>
          <div className="rounded-md border border-cream-300 bg-cream-50 px-3 py-2"><dt className="text-xs text-ink-500">{t('continuity.timing')}</dt><dd>{guard.lease_seconds} + {guard.takeover_margin_seconds} s · {t('continuity.every', { s: guard.tick_seconds })}</dd></div>
          <div className="rounded-md border border-cream-300 bg-cream-50 px-3 py-2"><dt className="text-xs text-ink-500">{t('continuity.order')}</dt><dd className="truncate" title={(guard.order || []).join(', ')}>{(guard.order || []).map(o => o.split('@').pop()).join(' → ')}</dd></div>
        </dl>
      )}
      {fences.length > 0 && (
        <ul className="space-y-1 text-xs">
          {fences.map(f => (
            <li key={f.host} className="flex flex-wrap items-center gap-1.5">
              <Badge tone={f.mandate_present && f.timer_active ? 'green' : 'red'}>{f.mandate_present && f.timer_active ? t('continuity.fenceOn') : t('continuity.fenceOff')}</Badge>
              <span className="text-ink-700">{f.host}</span>
              {f.error && <span className="text-terra-600">{f.error}</span>}
            </li>
          ))}
        </ul>
      )}
      {unfenced.length > 0 && <Notice bad text={t('continuity.unfencedWarning', { hosts: unfenced.map(f => f.host).join(', ') })} />}
      {(guard?.incidents || []).length > 0 && (
        <div>
          <p className="mb-1 text-xs font-medium text-ink-500">{t('continuity.incidents')}</p>
          <ul className="space-y-1 text-xs text-ink-700">
            {guard.incidents.slice().reverse().map((i, n) => (
              <li key={n}><Badge tone={i.kind === 'lost_host_failover' || i.kind === 'host_reintegrated' ? 'amber' : 'red'}>{t(`continuity.kind.${i.kind}`)}</Badge> {new Date(i.at * 1000).toLocaleString()}{i.from ? ` · ${i.from} → ${i.to}` : ''}{i.host ? ` · ${i.host}` : ''}{i.copy_age_seconds != null ? ` · ${t('continuity.lost', { s: i.copy_age_seconds })}` : ''}{i.error ? ` · ${i.error}` : ''}</li>
            ))}
          </ul>
        </div>
      )}
      {!guarded && (
        <div className="grid gap-3 sm:grid-cols-3">
          <TextField label={t('continuity.lease')} value={lease} onChange={setLease} inputMode="numeric" />
          <TextField label={t('continuity.margin')} value={margin} onChange={setMargin} inputMode="numeric" />
          <TextField label={t('continuity.tick')} value={tick} onChange={setTick} inputMode="numeric" />
        </div>
      )}
      {!guarded && <p className="text-xs text-ink-500">{t('continuity.objectives', { rpo: status?.replication?.interval_seconds ?? '—', rto: Number.isFinite(rto) ? rto : '—' })}</p>}
      {!guarded && <Checkbox label={t('continuity.keepStale')} checked={keepStale} onChange={setKeepStale} />}
      <TextField label={t('common.authorization')} value={auth} onChange={setAuth} placeholder={t('common.authorizationPlaceholder')} />
      <div className="flex flex-wrap gap-2">
        {guarded
          ? <Button kind="danger" icon={ShieldOff} disabled={!!busy || !canAct} onClick={() => setConfirmOff(true)}>{t('continuity.unguard')}</Button>
          : <Button kind="primary" icon={ShieldCheck} disabled={!!busy || !canAct || !status?.replication} onClick={() => act('guard')}>{busy === 'guard' ? t('continuity.arming') : t('continuity.guard')}</Button>}
      </div>
      {!status?.replication && <p className="text-xs text-ink-500">{t('continuity.needsReplication')}</p>}
      <Notice bad text={error} />
      <ConfirmModal open={confirmOff} danger pending={busy === 'unguard'} onCancel={() => setConfirmOff(false)} onConfirm={() => act('unguard')}
        title={t('continuity.unguardTitle')} message={t('continuity.unguardMessage')} confirmLabel={t('continuity.unguard')} />
    </section>
  )
}
