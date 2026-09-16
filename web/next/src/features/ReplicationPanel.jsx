import { useCallback, useEffect, useRef, useState } from 'react'
import { Copy, Play, Square, Save, RefreshCw, LogIn } from 'lucide-react'
import toast from 'react-hot-toast'
import Select from '../components/Select'
import TextField from '../components/TextField'
import Notice from '../components/Notice'
import Badge from '../components/Badge'
import Button from '../components/Button'
import TakeoverModal from './TakeoverModal'
import { useI18n } from '../store/useLocaleStore'
import { useSession } from '../store/useSessionStore'
import { getReplication, postReplication } from '../api/console'
import { INTERVALS, CAPTURES, ago, configureBody, replicationActionBody, standbyOf } from '../model/replication'

// A universe's replication to standby hosts, from the drawer: where it replicates, in which mode,
// run it now, run it on a schedule, stop; the copy each standby holds; and, on a standby that holds
// one, the takeover. A stopped run stops the universe for its capture; a live run checkpoints it
// with its memory and resumes it in place. The note says which, and what the last run did to it.
export default function ReplicationPanel({ row, onChanged }) {
  const { t } = useI18n()
  const [status, setStatus] = useState(null)
  const [loadError, setLoadError] = useState('')
  const [error, setError] = useState('')
  const [busy, setBusy] = useState('')
  const [target, setTarget] = useState('all')
  const [interval, setInterval_] = useState(900)
  const [capture, setCapture] = useState('live')
  const [auth, setAuth] = useState('')
  const [takeover, setTakeover] = useState(null)   // {standby, copy}
  const [lastTakeover, setLastTakeover] = useState(null)
  const takeoverKey = useRef(0)   // a new key per opening: the modal starts clean, and keeps its exit animation on close
  const sessionHosts = useSession(s => s.session?.hosts) || []
  const hostId = row.host.id, universe = row.uuid

  const load = useCallback(async () => {
    setBusy(b => b || 'load')
    try {
      const d = await getReplication(hostId, universe)
      setStatus(d); setLoadError('')
      if (d.replication) {
        setTarget(d.replication.mode === 'all' ? 'all' : Number(d.replication.mode))
        if (d.replication.interval_seconds) setInterval_(d.replication.interval_seconds)
        setCapture(d.replication.capture || 'stopped')
      }
    } catch (e) {
      setLoadError(e?.error || t('replication.unavailable'))
      if (Array.isArray(e?.candidates)) setStatus(s => s || { candidates: e.candidates })
    } finally { setBusy(b => (b === 'load' ? '' : b)) }
  }, [hostId, universe, t])
  useEffect(() => { load() }, [load])

  async function act(action) {
    setBusy(action); setError('')
    try {
      const body = action === 'configure'
        ? configureBody({ host: hostId, universe_uuid: universe, authorization_ref: auth, standbys: target, interval_seconds: interval, capture })
        : replicationActionBody({ action, host: hostId, universe_uuid: universe, authorization_ref: auth })
      const report = await postReplication(body)
      if (report?.result === 'refused' || report?.result === 'unknown' || report?.error) throw { error: report.error || t('common.refused') }
      toast.success(t(`replication.toast.${action}`))
      await load()
      onChanged?.()
    } catch (e) {
      const code = e instanceof Error ? e.message : null
      setError(code ? t(`validation.${code}`) : e?.error || e?.message || t('common.refused'))
    } finally { setBusy('') }
  }

  const rep = status?.replication, candidates = status?.candidates || [], armed = !!status?.schedule?.armed, last = status?.last_run
  const live = (rep?.capture || capture) === 'live'
  const canAct = row.host.allowActions
  const targets = [{ value: 'all', label: t('replication.allHosts', { n: candidates.length }) }, ...candidates.map((_, i) => ({ value: i + 1, label: t('replication.nHosts', { n: i + 1 }), hint: t('replication.mostMemory') }))]
  const captures = CAPTURES.map(v => ({ value: v, label: t(`replication.capture.${v}`), hint: t(`replication.capture.${v}Hint`) }))
  const intervals = INTERVALS.map(v => ({ value: v, label: t(`replication.every.${v}`) }))
  const nowSeconds = Math.round(Date.now() / 1000)

  return (
    <section className="space-y-4" aria-labelledby="replication-title">
      <div className="flex flex-wrap items-center gap-2">
        <h3 id="replication-title" className="text-sm font-semibold text-ink-900">{t('replication.title')}</h3>
        <Badge tone={armed ? 'green' : 'muted'}>{armed ? t('replication.scheduled') : t('replication.notScheduled')}</Badge>
        {rep && <Badge tone={live ? 'green' : 'amber'}>{live ? t('replication.live') : t('replication.stopped')}</Badge>}
        <button type="button" onClick={load} disabled={busy === 'load'} aria-label={t('replication.refresh')} className="ml-auto rounded-md p-1.5 text-ink-500 hover:bg-cream-200 hover:text-green-600 disabled:opacity-50"><RefreshCw size={15} className={busy === 'load' ? 'animate-spin' : ''} /></button>
      </div>
      {!status && !loadError && <p className="text-sm text-ink-500">{t('replication.reading')}</p>}
      {loadError && <Notice bad text={loadError} />}
      {status && (
        <>
          <p className="text-sm text-ink-700">
            {live ? t('replication.liveNote') : t('replication.stoppedNote')}
            {last?.stopped_for_seconds != null && <> {last.capture === 'live' ? t('replication.lastInterrupted', { s: last.stopped_for_seconds }) : t('replication.lastStopped', { s: last.stopped_for_seconds })}</>}
          </p>
          <div className="grid gap-3 sm:grid-cols-3">
            <div><p className="mb-1.5 text-xs font-medium text-ink-500">{t('replication.mode')}</p><Select value={capture} onChange={setCapture} options={captures} label={t('replication.mode')} disabled={!canAct} /></div>
            <div><p className="mb-1.5 text-xs font-medium text-ink-500">{t('replication.target')}</p><Select value={target} onChange={setTarget} options={targets} label={t('replication.target')} disabled={!canAct} searchPlaceholder={t('common.search')} noResult={t('common.noMatch')} clearSearchLabel={t('common.clearSearch')} /></div>
            <div><p className="mb-1.5 text-xs font-medium text-ink-500">{t('replication.schedule')}</p><Select value={interval} onChange={setInterval_} options={intervals} label={t('replication.schedule')} disabled={!canAct} searchPlaceholder={t('common.search')} noResult={t('common.noMatch')} clearSearchLabel={t('common.clearSearch')} /></div>
          </div>
          <TextField label={t('common.authorization')} value={auth} onChange={setAuth} placeholder={t('common.authorizationPlaceholder')} />
          <div className="flex flex-wrap gap-2">
            <Button icon={Save} disabled={!!busy || !canAct} onClick={() => act('configure')}>{busy === 'configure' ? t('replication.saving') : rep ? t('replication.save') : t('replication.setUp')}</Button>
            <Button icon={Copy} disabled={!!busy || !rep || !canAct} onClick={() => act('run')}>{busy === 'run' ? t('replication.replicating') : t('replication.runNow')}</Button>
            {armed
              ? <Button icon={Square} disabled={!!busy || !canAct} onClick={() => act('stop')}>{busy === 'stop' ? t('replication.stopping') : t('replication.stopSchedule')}</Button>
              : <Button kind="primary" icon={Play} disabled={!!busy || !rep || !canAct} onClick={() => act('start')}>{busy === 'start' ? t('replication.starting') : t('replication.startSchedule')}</Button>}
          </div>
          {!canAct && <p className="text-xs text-ink-500">{t('universe.actionsDisabled')}</p>}
          <Notice bad text={error} />
          {rep && (
            <p className="text-xs text-ink-500">
              {t('replication.targetLine', { n: rep.standbys?.length || 0, why: rep.chosen_because || '' })}
              {last && <> {t('replication.lastRun', { when: ago(nowSeconds - last.at, t) })}: {last.ok ? 'ok' : `${t('common.refused')} — ${last.error || ''}`}.</>}
            </p>
          )}
          {status.standbys?.length > 0 && (
            <div className="overflow-x-auto rounded-md border border-cream-300">
              <table className="w-full min-w-[32rem] text-sm">
                <thead className="bg-cream-100 text-left text-xs uppercase tracking-wide text-ink-500">
                  <tr><th className="px-3 py-2">{t('replication.standby')}</th><th className="px-3 py-2">{t('replication.copy')}</th><th className="px-3 py-2">{t('replication.age')}</th><th className="px-3 py-2">{t('replication.onHost')}</th><th className="px-3 py-2" /></tr>
                </thead>
                <tbody>
                  {status.standbys.map(s => {
                    const standby = standbyOf(s, candidates)
                    const present = !!s.copy?.present_on_host
                    const standbyAllows = !!sessionHosts.find(h => h.id === standby.id)?.allowActions
                    const why = !standby.id ? t('takeover.unknownStandby') : !standbyAllows ? t('takeover.readOnlyStandby') : ''
                    return (
                      <tr key={s.host} className="border-t border-cream-300">
                        <td className="px-3 py-2 text-ink-900">{standby.name}</td>
                        <td className="px-3 py-2">{s.error ? <span className="text-terra-600">{s.error}</span> : s.copy ? <span className="inline-flex flex-wrap items-center gap-1.5">{t('replication.generation', { n: s.copy.generation })}<Badge tone={s.copy.capture === 'live' ? 'green' : 'amber'}>{s.copy.capture === 'live' ? t('replication.live') : t('replication.stopped')}</Badge></span> : <span className="text-ink-500">{t('replication.noneYet')}</span>}</td>
                        <td className="px-3 py-2 text-ink-700">{s.copy ? ago(s.copy.age_seconds, t) : '—'}</td>
                        <td className="px-3 py-2">{s.copy ? <Badge tone={present ? 'green' : 'red'}>{present ? t('replication.present') : t('replication.missing')}</Badge> : '—'}</td>
                        <td className="px-3 py-2 text-right">
                          {!s.error && (
                            <Button kind="danger" icon={LogIn} className="!px-2 !py-1 text-xs" disabled={!canAct || !!why || !!busy} title={why} onClick={() => { takeoverKey.current += 1; setTakeover({ standby, copy: s.copy }) }}>{t('takeover.here')}</Button>
                          )}
                        </td>
                      </tr>
                    )
                  })}
                </tbody>
              </table>
            </div>
          )}
          {lastTakeover && <Notice text={t('takeover.inPlace', { to: lastTakeover.toName || lastTakeover.to, capture: lastTakeover.capture === 'live' ? t('takeover.doneMemory') : t('takeover.doneAfresh') })} />}
        </>
      )}
      <TakeoverModal key={takeoverKey.current} open={!!takeover} row={row} standby={takeover?.standby} copy={takeover?.copy} onClose={() => setTakeover(null)}
        onDone={report => { setLastTakeover(report); load(); onChanged?.() }} />
    </section>
  )
}
