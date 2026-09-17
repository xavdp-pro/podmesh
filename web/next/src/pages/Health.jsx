import { useCallback, useEffect, useState } from 'react'
import { Cpu, MemoryStick, HardDrive, RefreshCw, AlertTriangle, CheckCircle2, CircleDashed } from 'lucide-react'
import Badge from '../components/Badge'
import Button from '../components/Button'
import Notice from '../components/Notice'
import { useI18n } from '../store/useLocaleStore'
import { useSession } from '../store/useSessionStore'
import { getHealth } from '../api/console'
import { replicationSummary, linkState, ratio, bytes, STORE_WARN_BYTES } from '../model/health'
import { ago } from '../model/replication'

const short = s => (s ? String(s).slice(0, 8) : '—')
const TONE = { healthy: 'green', degraded: 'amber', failing: 'red', absent: 'muted' }

function Bar({ value, label }) {
  const pct = value == null ? 0 : Math.round(value * 100)
  const tone = pct >= 90 ? 'bg-terra-600' : pct >= 70 ? 'bg-amber-600' : 'bg-green-600'
  return <div className="h-1.5 w-full overflow-hidden rounded-full bg-cream-300" role="meter" aria-label={label} aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}><div className={`h-full rounded-full ${tone}`} style={{ width: `${pct}%` }} /></div>
}

// A universe's replication at a glance: the manager replicates itself peer to peer (the links below);
// an ordinary universe by warm-standby runs set up from its drawer, summarised from the workstation's ledger.
function ReplicationCell({ universe, replication: r }) {
  const { t } = useI18n()
  if (universe.manager) return <span className="text-xs text-ink-500">{t('health.peerToPeer')}</span>
  if (!r) return <Badge tone="muted">{t('health.none')}</Badge>
  const guardBadge = r.guarded ? <Badge tone={r.guard_state === 'failing_over' ? 'red' : 'green'}>{r.guard_state === 'failing_over' ? t('continuity.failingOver') : t('continuity.guarded')}</Badge> : <Badge tone="muted">{t('continuity.unguarded')}</Badge>
  const failed = r.last_run && !r.last_run.ok
  return (
    <div className="space-y-0.5">
      <span className="flex flex-wrap gap-1"><Badge tone={failed ? 'red' : r.armed ? 'green' : 'amber'}>{failed ? t('health.lastRunFailed') : r.armed ? t('replication.scheduled') : t('health.manual')}</Badge>{guardBadge}</span>
      <p className="text-xs text-ink-500">
        {r.mode === 'all' ? t('health.allStandbys', { n: r.standbys }) : t('health.nStandbys', { n: r.standbys })}
        {r.armed && r.interval_seconds ? ` · ${t(`replication.every.${r.interval_seconds}`) !== `replication.every.${r.interval_seconds}` ? t(`replication.every.${r.interval_seconds}`) : `${r.interval_seconds} s`}` : ''}
        {` · ${t('health.lastCopy', { when: r.last_copy_age_seconds == null ? t('time.never') : ago(r.last_copy_age_seconds, t) })}`}
        {r.capture === 'live' ? ` · ${t('replication.live')}` : ''}
        {r.last_run?.stopped_for_seconds != null ? ` · ${r.last_run.capture === 'live' ? t('health.interrupted', { s: r.last_run.stopped_for_seconds }) : t('health.stoppedFor', { s: r.last_run.stopped_for_seconds })}` : ''}
      </p>
      {failed && <p className="text-xs text-terra-600">{r.last_run.error}</p>}
      {r.last_incident && <p className="text-xs text-amber-600">{t(`continuity.kind.${r.last_incident.kind}`)} · {new Date(r.last_incident.at * 1000).toLocaleString()}</p>}
    </div>
  )
}

export default function Health() {
  const { t } = useI18n()
  const token = useSession(s => s.session?.token)
  const [data, setData] = useState(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const load = useCallback(async () => {
    setBusy(true)
    try { setData(await getHealth()); setError('') } catch (e) { setError(e?.error || t('health.unavailable')) } finally { setBusy(false) }
  }, [t])
  useEffect(() => { if (!token) return undefined; load(); const timer = setInterval(load, 15000); return () => clearInterval(timer) }, [token, load])

  if (!token) return <p className="text-sm text-ink-500">{t('layout.waitingSession')}</p>
  if (!data) return <div className="space-y-2"><h1 className="text-xl font-semibold text-ink-900">{t('health.title')}</h1><p className="text-sm text-ink-500">{error || t('health.reading')}</p></div>
  const hosts = data.hosts, rep = replicationSummary(hosts)
  const Panel = ({ children, className = '' }) => <section className={`min-w-0 rounded-xl border border-cream-300 bg-cream-50 p-4 ${className}`}>{children}</section>
  const th = 'px-3 py-2 text-left text-xs font-medium uppercase tracking-wide text-ink-500'
  const td = 'px-3 py-2 align-top'

  return (
    <div className="space-y-6">
      <Panel>
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div>
            <h1 className="flex flex-wrap items-center gap-2 text-xl font-semibold text-ink-900">{t('health.title')} <Badge tone={TONE[rep.state]}>{t(`health.state.${rep.state}`)}</Badge></h1>
            <p className="mt-1 text-sm text-ink-700">{t('health.lead', rep)}</p>
          </div>
          <Button icon={RefreshCw} onClick={load} disabled={busy}>{busy ? t('common.reading') : t('common.refresh')}</Button>
        </div>
        {rep.storeWarning && <p className="mt-3 flex items-start gap-2 text-sm text-amber-600"><AlertTriangle size={16} className="mt-0.5 shrink-0" />{t('health.storeWarning', { size: bytes(rep.largestStoreBytes), limit: bytes(STORE_WARN_BYTES) })}</p>}
        {error && <div className="mt-3"><Notice bad text={error} /></div>}
      </Panel>

      <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
        {hosts.map(h => {
          const hs = h.host
          const mem = hs ? ratio(hs.memory_total_bytes - hs.memory_available_bytes, hs.memory_total_bytes) : null
          const disk = hs ? ratio(hs.storage.used_bytes, hs.storage.size_bytes) : null
          const load1 = hs ? ratio(hs.load_average['1m'], hs.cpu_count) : null
          return (
            <Panel key={h.id}>
              <h2 className="text-base font-semibold text-ink-900">{h.name}</h2>
              {Object.entries(h.errors || {}).map(([k, v]) => <p key={k} className="mt-1 text-xs text-terra-600">{k}: {v}</p>)}
              {hs && (
                <dl className="mt-3 space-y-3 text-sm">
                  <div><dt className="flex items-center gap-1.5 text-ink-500"><Cpu size={14} /> {t('health.cpu')}</dt><dd className="mt-1 space-y-1"><Bar value={load1} label={t('health.loadPerCore')} /><span className="text-xs text-ink-500">{t('health.cores', { n: hs.cpu_count, load: hs.load_average['1m']?.toFixed(2) })}</span></dd></div>
                  <div><dt className="flex items-center gap-1.5 text-ink-500"><MemoryStick size={14} /> {t('health.memory')}</dt><dd className="mt-1 space-y-1"><Bar value={mem} label={t('health.memoryUsed')} /><span className="text-xs text-ink-500">{t('health.of', { used: bytes(hs.memory_total_bytes - hs.memory_available_bytes), total: bytes(hs.memory_total_bytes) })}</span></dd></div>
                  <div><dt className="flex items-center gap-1.5 text-ink-500"><HardDrive size={14} /> {t('health.disk')}</dt><dd className="mt-1 space-y-1"><Bar value={disk} label={t('health.diskUsed')} /><span className="text-xs text-ink-500">{t('health.of', { used: bytes(hs.storage.used_bytes), total: bytes(hs.storage.size_bytes) })} · {hs.storage.backend}{hs.storage.dedicated ? ` · ${t('health.dedicated')}` : ` · ${t('health.sharedWithSystem')}`} · {t('health.growth', { g: hs.storage.growth })}</span></dd></div>
                </dl>
              )}
            </Panel>
          )
        })}
      </div>

      <Panel>
        <h2 className="text-base font-semibold text-ink-900">{t('health.universes')}</h2>
        <p className="mt-1 text-sm text-ink-700">{t('health.universesLead')}</p>
        {data.replicationError && <p className="mt-1 text-xs text-terra-600">{data.replicationError}</p>}
        <div className="-mx-4 mt-3 overflow-x-auto px-4">
          <table className="w-full min-w-[48rem] text-sm">
            <thead><tr><th className={th}>{t('health.universe')}</th><th className={th}>{t('universe.host')}</th><th className={th}>{t('universe.state')}</th><th className={th}>{t('health.cpu')}</th><th className={th}>{t('health.memory')}</th><th className={th}>{t('health.diskWritten')}</th><th className={th}>{t('replication.title')}</th></tr></thead>
            <tbody>
              {hosts.flatMap(h => (h.universes || []).map(u => (
                <tr key={h.id + u.universe_uuid} className="border-t border-cream-300">
                  <td className={td}><code className="text-xs">{short(u.universe_uuid)}</code>{u.manager && <Badge tone="muted" className="ml-1">manager</Badge>}</td>
                  <td className={td}>{h.name}</td>
                  <td className={td}><Badge tone={u.state === 'running' ? 'green' : u.state === 'paused' ? 'amber' : 'muted'}>{u.state}</Badge></td>
                  <td className={`${td} min-w-[8rem]`}>{u.cpu_percent_of_one_core == null ? '—' : <><Bar value={ratio(u.cpu_percent_of_one_core, 100 * (u.cpus_allowed || h.host?.cpu_count || 1))} label="cpu" /><span className="text-xs text-ink-500">{u.cpu_percent_of_one_core} %{u.cpus_allowed ? ` / ${u.cpus_allowed}` : ''}</span></>}</td>
                  <td className={`${td} min-w-[8rem]`}>{u.memory_current_bytes == null ? '—' : <><Bar value={ratio(u.memory_current_bytes, u.memory_max_bytes || h.host?.memory_total_bytes)} label="memory" /><span className="text-xs text-ink-500">{bytes(u.memory_current_bytes)}{u.memory_max_bytes ? ` / ${bytes(u.memory_max_bytes)}` : ` (${t('health.noLimit')})`}</span></>}</td>
                  <td className={td}>{bytes(u.disk_written_bytes)}</td>
                  <td className={td}><ReplicationCell universe={u} replication={data.replication?.[u.universe_uuid]} /></td>
                </tr>
              )))}
            </tbody>
          </table>
        </div>
      </Panel>

      <Panel>
        <h2 className="text-base font-semibold text-ink-900">{t('health.links')}</h2>
        <p className="mt-1 text-sm text-ink-700">{t('health.linksLead')}</p>
        <div className="-mx-4 mt-3 overflow-x-auto px-4">
          <table className="w-full min-w-[44rem] text-sm">
            <thead><tr><th className={th}>{t('health.from')}</th><th className={th}>{t('health.to')}</th><th className={th}>{t('universe.state')}</th><th className={th}>{t('health.lastSuccess')}</th><th className={th}>{t('health.failures')}</th><th className={th}>{t('health.acknowledged')}</th><th className={th}>{t('health.store')}</th></tr></thead>
            <tbody>
              {hosts.flatMap(h => (h.managers || []).flatMap(m => m.error
                ? [<tr key={h.id + m.universe_uuid} className="border-t border-cream-300"><td className={td}>{h.name}</td><td colSpan={6} className={`${td} text-terra-600`}>{m.error}</td></tr>]
                : m.links.map(l => {
                  const s = linkState(l)
                  const Icon = s === 'healthy' ? CheckCircle2 : s === 'degraded' ? CircleDashed : AlertTriangle
                  return (
                    <tr key={h.id + l.peer} className="border-t border-cream-300">
                      <td className={td}>{h.name} <code className="text-xs">{short(m.replica_id)}</code></td>
                      <td className={td}><code className="text-xs">{short(l.peer)}</code></td>
                      <td className={td}><Badge tone={TONE[s]}><Icon size={12} /> {t(`health.state.${s}`)}</Badge><p className="text-xs text-ink-500">{l.outcome}</p></td>
                      <td className={td}>{l.last_success_age_ms == null ? t('time.never') : ago(l.last_success_age_ms / 1000, t)}</td>
                      <td className={td}>{l.failures} <span className="text-xs text-ink-500">/ {l.successes} ok</span></td>
                      <td className={td}>{l.acknowledged_history_len ?? '—'}</td>
                      <td className={`${td} ${m.store_bytes >= STORE_WARN_BYTES ? 'text-amber-600' : ''}`}>{bytes(m.store_bytes)}</td>
                    </tr>
                  )
                })))}
            </tbody>
          </table>
        </div>
      </Panel>
    </div>
  )
}
