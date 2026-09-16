import { useEffect, useState } from 'react'
import { NavLink, Outlet } from 'react-router-dom'
import { Boxes, Activity, TerminalSquare, RefreshCw, Menu, X } from 'lucide-react'
import LanguageSwitcher from './LanguageSwitcher'
import Badge from './Badge'
import { useI18n } from '../store/useLocaleStore'
import { useSession } from '../store/useSessionStore'
import { useSnapshot, POLL_MS, STALE_AFTER_MS } from '../store/useSnapshotStore'
import { hostSummary } from '../model/universes'
import { ago } from '../model/replication'

const NAV = [
  { to: '/', key: 'layout.universes', icon: Boxes, end: true },
  { to: '/health', key: 'layout.health', icon: Activity },
  { to: '/run', key: 'layout.run', icon: TerminalSquare },
]

// The frame of every page: the brand, the three views, the hosts' reachability from the last
// collection, the language. The snapshot is read on arrival and every thirty seconds.
export default function Layout() {
  const { t } = useI18n()
  const token = useSession(s => s.session?.token)
  const failure = useSession(s => s.failure)
  const { snapshot, busy, now, refresh, tick } = useSnapshot()
  const [open, setOpen] = useState(false)
  useEffect(() => {
    if (!token) return undefined
    refresh()
    const poll = setInterval(refresh, POLL_MS)
    const clock = setInterval(tick, 5000)
    return () => { clearInterval(poll); clearInterval(clock) }
  }, [token, refresh, tick])
  const summary = hostSummary(snapshot?.hosts)
  const stale = !!snapshot && now - snapshot.receivedAt > STALE_AFTER_MS
  const link = ({ isActive }) => `flex items-center gap-2 rounded-md px-3 py-2 text-sm font-medium transition ${isActive ? 'bg-green-100 text-green-700' : 'text-ink-700 hover:bg-cream-200'}`
  return (
    <div className="min-h-dvh">
      <header className="sticky top-0 z-40 border-b border-cream-300 bg-cream-50/95 backdrop-blur">
        <div className="mx-auto flex max-w-7xl items-center gap-3 px-4 py-3">
          <span className="text-sm font-bold tracking-widest text-ink-900">{t('brand')}</span>
          <nav className="hidden items-center gap-1 md:flex" aria-label={t('layout.menu')}>
            {NAV.map(n => <NavLink key={n.to} to={n.to} end={n.end} className={link}><n.icon size={15} />{t(n.key)}</NavLink>)}
          </nav>
          <div className="ml-auto hidden items-center gap-2 text-xs text-ink-500 sm:flex">
            {snapshot && <Badge tone={summary.responding === summary.total ? 'green' : 'red'}>{t('layout.hosts', summary)}</Badge>}
            {snapshot && <span className={stale ? 'text-terra-600' : ''}>{t('layout.collected', { when: ago((now - snapshot.receivedAt) / 1000, t) })}</span>}
            <button type="button" onClick={refresh} disabled={busy || !token} aria-label={t('common.refresh')} className="rounded-md p-1.5 hover:bg-cream-200 hover:text-green-600 disabled:opacity-50"><RefreshCw size={15} className={busy ? 'animate-spin' : ''} /></button>
          </div>
          <LanguageSwitcher className="hidden w-32 sm:block" />
          <button type="button" onClick={() => setOpen(v => !v)} aria-label={t('layout.menu')} aria-expanded={open} className="ml-auto rounded-md p-1.5 text-ink-700 hover:bg-cream-200 md:hidden">{open ? <X size={18} /> : <Menu size={18} />}</button>
        </div>
        {open && (
          <div className="space-y-2 border-t border-cream-300 px-4 py-3 md:hidden">
            <nav className="flex flex-col gap-1">{NAV.map(n => <NavLink key={n.to} to={n.to} end={n.end} className={link} onClick={() => setOpen(false)}><n.icon size={15} />{t(n.key)}</NavLink>)}</nav>
            <div className="flex flex-wrap items-center gap-2 text-xs text-ink-500">
              {snapshot && <Badge tone={summary.responding === summary.total ? 'green' : 'red'}>{t('layout.hosts', summary)}</Badge>}
              <button type="button" onClick={refresh} disabled={busy || !token} className="rounded-md border border-cream-400 px-2 py-1">{t('common.refresh')}</button>
              <LanguageSwitcher />
            </div>
          </div>
        )}
      </header>
      <main className="mx-auto max-w-7xl px-4 py-6">
        {failure && <p role="alert" className="mb-4 rounded-md border border-terra-600/30 bg-terra-100 px-3 py-2 text-sm text-terra-600">{t('layout.serverDown', { why: failure })}</p>}
        {stale && <p role="status" className="mb-4 rounded-md border border-amber-600/30 bg-amber-100 px-3 py-2 text-sm text-amber-600">{t('layout.stale')}</p>}
        {token ? <Outlet /> : !failure && <p className="text-sm text-ink-500">{t('layout.waitingSession')}</p>}
      </main>
      <footer className="mx-auto flex max-w-7xl flex-wrap items-center gap-3 px-4 pb-6 text-xs text-ink-500">
        <span>{t('layout.localOnly')}</span>
        <a href="/" className="underline hover:text-green-600">{t('layout.classic')}</a>
      </footer>
    </div>
  )
}
