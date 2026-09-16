import { useEffect, useRef, useState } from 'react'
import { Play, Pause, Square, Gauge, ArrowRightLeft, RefreshCw } from 'lucide-react'
import SlideOver from '../components/SlideOver'
import Badge, { stateTone } from '../components/Badge'
import Button from '../components/Button'
import Notice from '../components/Notice'
import ActionModal from './ActionModal'
import ReplicationPanel from './ReplicationPanel'
import { useI18n } from '../store/useLocaleStore'
import { getDetails } from '../api/console'
import { ACTIONS } from '../model/universes'
import { detailResources } from '../model/details'

const ICONS = { start: Play, pause: Pause, resume: Play, stop: Square, resources: Gauge, move: ArrowRightLeft }

// A container's observed configuration and metrics, read from the host when the drawer opens.
function Details({ row }) {
  const { t } = useI18n()
  const [data, setData] = useState(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [revision, setRevision] = useState(0)
  useEffect(() => {
    let mounted = true
    setBusy(true); setError('')
    getDetails(row.host.id, [row.containerId])
      .then(body => { if (!mounted) return; if (!body?.ok) throw { error: body?.error || t('details.unavailable') }; setData(body.data) })
      .catch(e => { if (mounted) setError(e?.error || t('details.unavailable')) })
      .finally(() => { if (mounted) setBusy(false) })
    return () => { mounted = false }
  }, [row.host.id, row.containerId, revision, t])
  const phrase = v => `${v.text ?? t(v.key)}${v.suffix || ''}`
  return (
    <section className="space-y-2">
      <div className="flex items-center gap-2">
        <h3 className="text-sm font-semibold text-ink-900">{t('details.title')}</h3>
        {data && <span className="text-xs text-ink-500">{data.observation_source} · {new Date(data.observed_at * 1000).toLocaleTimeString()}</span>}
        <button type="button" onClick={() => setRevision(r => r + 1)} disabled={busy} aria-label={t('details.refresh')} className="ml-auto rounded-md p-1.5 text-ink-500 hover:bg-cream-200 hover:text-green-600 disabled:opacity-50"><RefreshCw size={15} className={busy ? 'animate-spin' : ''} /></button>
      </div>
      {busy && !data && <p className="text-sm text-ink-500" role="status">{t('details.reading')}</p>}
      <Notice bad text={error} />
      {data && (
        <dl className="grid grid-cols-2 gap-2 sm:grid-cols-3">
          {detailResources(data).map(([k, v]) => <div key={k} className="rounded-md border border-cream-300 bg-cream-100 px-3 py-2"><dt className="text-xs text-ink-500">{t(k)}</dt><dd className="truncate text-sm text-ink-900" title={phrase(v)}>{phrase(v)}</dd></div>)}
        </dl>
      )}
      {data?.metrics?.status && data.metrics.status !== 'observed' && data.metrics.reason && <p className="text-xs text-ink-500">{data.metrics.reason}</p>}
    </section>
  )
}

// One universe: its facts, its observed resources, the lifecycle actions the host allows, and its
// replication. Every action opens a modal that names the target and asks for the mandate.
export default function UniverseDrawer({ row, hosts, stale, onClose, onChanged }) {
  const { t } = useI18n()
  const [op, setOp] = useState(null)
  const actionKey = useRef(0)
  const open = !!row
  const canAct = !!row?.uuid && !!row?.host.allowActions && !stale
  return (
    <>
      <SlideOver open={open} onClose={onClose} title={row?.name || ''} subtitle={row ? `${row.host.name} · ${row.uuid || t('universe.unmanaged')}` : ''}>
        {row && (
          <div className="space-y-6">
            <dl className="grid grid-cols-[7rem_1fr] gap-x-4 gap-y-1.5 text-sm">
              <dt className="text-ink-500">{t('universe.host')}</dt><dd className="text-ink-900">{row.host.name}</dd>
              <dt className="text-ink-500">{t('universe.state')}</dt><dd><Badge tone={stateTone(row.state)}>{row.state}</Badge></dd>
              <dt className="text-ink-500">{t('universe.uuid')}</dt><dd className="break-all"><code className="text-xs text-ink-900">{row.uuid || t('universe.unmanaged')}</code></dd>
              <dt className="text-ink-500">{t('universe.container')}</dt><dd className="break-all"><code className="text-xs text-ink-900">{row.containerId}</code></dd>
              <dt className="text-ink-500">{t('universe.image')}</dt><dd className="break-all text-ink-900">{row.image || '—'}</dd>
            </dl>
            <Details key={row.key} row={row} />
            <section className="space-y-2">
              <h3 className="text-sm font-semibold text-ink-900">{t('actions.title2')}</h3>
              <div className="flex flex-wrap gap-2">
                {ACTIONS.map(a => (
                  <Button key={a.op} icon={ICONS[a.op]} kind={a.danger ? 'danger' : 'plain'} disabled={!canAct || !a.applies(row.state) || (a.op === 'move' && !row.host.canMove)}
                    title={!a.applies(row.state) ? t('actions.notFor', { state: row.state }) : ''} onClick={() => { actionKey.current += 1; setOp(a.op) }}>{t(`actions.${a.op}.label`)}</Button>
                ))}
              </div>
              {!row.uuid && <p className="text-xs text-ink-500">{t('universe.unmanagedNote')}</p>}
              {row.uuid && !row.host.allowActions && <p className="text-xs text-ink-500">{t('universe.actionsDisabled')}</p>}
              {stale && <p className="text-xs text-terra-600">{t('layout.stale')}</p>}
              <p className="text-xs text-ink-500">{t('universe.ownership')}</p>
            </section>
            {row.uuid && <ReplicationPanel key={row.key} row={row} onChanged={onChanged} />}
            <details className="text-xs text-ink-500">
              <summary className="cursor-pointer">{t('universe.rawEvidence')}</summary>
              <pre className="mt-2 max-h-64 overflow-auto whitespace-pre-wrap break-all rounded-md border border-cream-300 bg-cream-100 p-2">{JSON.stringify(row.raw, null, 2)}</pre>
            </details>
          </div>
        )}
      </SlideOver>
      <ActionModal key={actionKey.current} row={row} op={op} hosts={hosts} onClose={() => setOp(null)} onDone={onChanged} />
    </>
  )
}
