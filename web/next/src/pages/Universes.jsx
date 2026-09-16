import { useMemo, useState } from 'react'
import { Server, Boxes, Search, X } from 'lucide-react'
import Badge, { stateTone } from '../components/Badge'
import UniverseDrawer from '../features/UniverseDrawer'
import { useI18n } from '../store/useLocaleStore'
import { useSnapshot, STALE_AFTER_MS } from '../store/useSnapshotStore'
import { useSession } from '../store/useSessionStore'
import { universeRows, hostReach, matches, short } from '../model/universes'

// Every universe, per host, from the gateway's last collection. One click opens its drawer.
export default function Universes() {
  const { t } = useI18n()
  const { snapshot, busy, now, refresh } = useSnapshot()
  const hosts = useSession(s => s.session?.hosts) || []
  const [query, setQuery] = useState('')
  const [selectedKey, setSelectedKey] = useState(null)
  const rows = useMemo(() => universeRows(snapshot?.hosts), [snapshot])
  const selected = rows.find(r => r.key === selectedKey) || null
  const stale = !!snapshot && now - snapshot.receivedAt > STALE_AFTER_MS
  const snapshotHosts = snapshot?.hosts || []

  return (
    <section className="space-y-6">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-end sm:justify-between">
        <div>
          <h1 className="text-xl font-semibold text-ink-900">{t('universes.title')}</h1>
          <p className="mt-1 text-sm text-ink-700">{t('universes.lead')}</p>
        </div>
        <div className="relative w-full sm:w-72">
          <Search size={15} className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-ink-500" />
          <input value={query} onChange={e => setQuery(e.target.value)} placeholder={t('universes.search')} aria-label={t('universes.search')} spellCheck={false}
            className="w-full rounded-md border border-cream-400 bg-cream-50 py-2 pl-9 pr-9 text-sm focus:border-green-600 focus:outline-none focus:ring-2 focus:ring-green-600/20" />
          {query && <button type="button" onClick={() => setQuery('')} aria-label={t('common.clearSearch')} className="absolute right-2 top-1/2 -translate-y-1/2 rounded p-1 text-ink-500 hover:bg-cream-200 hover:text-ink-900"><X size={14} /></button>}
        </div>
      </div>

      {!snapshot && <p className="text-sm text-ink-500">{busy ? t('universes.reading') : t('universes.waiting')}</p>}

      <div className="grid gap-4 lg:grid-cols-2">
        {snapshotHosts.map(h => {
          const reach = hostReach(h)
          const mine = rows.filter(r => r.host.id === h.id && matches(r, query))
          const all = rows.filter(r => r.host.id === h.id)
          return (
            <article key={h.id} className="min-w-0 rounded-xl border border-cream-300 bg-cream-50 p-4">
              <div className="flex flex-wrap items-center gap-2">
                <span className="rounded-lg bg-cream-200 p-2 text-ink-700"><Server size={16} /></span>
                <div className="min-w-0 flex-1">
                  <h2 className="truncate text-base font-semibold text-ink-900">{h.name}</h2>
                  <p className="truncate text-xs text-ink-500"><code>{short(h.responses?.identity?.data?.host_uuid)}</code> · {h.responses?.capabilities?.data?.version || t('universes.versionUnknown')} · {t('universes.count', { n: all.length })}</p>
                </div>
                <Badge tone={reach.ok ? 'green' : 'red'}>{reach.ok ? t('layout.responding') : t('layout.needsAttention')}</Badge>
                {!h.allowActions && <Badge tone="muted">{t('universes.readOnly')}</Badge>}
              </div>
              {reach.errors.map((e, i) => <p key={i} className="mt-2 text-xs text-terra-600">{e}</p>)}
              <ul className="mt-3 divide-y divide-cream-300 border-t border-cream-300">
                {mine.map(r => (
                  <li key={r.key}>
                    <button type="button" onClick={() => setSelectedKey(r.key)} className="flex w-full items-center gap-3 px-1 py-2.5 text-left hover:bg-cream-100">
                      <span className="rounded-md bg-cream-200 p-1.5 text-ink-500"><Boxes size={14} /></span>
                      <span className="min-w-0 flex-1">
                        <span className="block truncate text-sm font-medium text-ink-900">{r.name}</span>
                        <span className="block truncate text-xs text-ink-500">{r.uuid ? `${short(r.uuid)} · ${r.image}` : t('universe.unmanaged')}</span>
                      </span>
                      <Badge tone={stateTone(r.state)}>{r.state}</Badge>
                    </button>
                  </li>
                ))}
                {!mine.length && <li className="px-1 py-3 text-sm text-ink-500">{all.length ? t('universes.noMatch') : reach.ok ? t('universes.none') : t('universes.noInventory')}</li>}
              </ul>
            </article>
          )
        })}
      </div>
      <UniverseDrawer row={selected} hosts={hosts} stale={stale} onClose={() => setSelectedKey(null)} onChanged={refresh} />
    </section>
  )
}
