import { NavLink, Outlet } from 'react-router-dom'
import { Users, Server, LogOut } from 'lucide-react'
import toast from 'react-hot-toast'
import { useI18n } from '../store/useLocaleStore'
import { useSession } from '../store/useSessionStore'
import { logout } from '../api/admin'
import LanguageSwitcher from './LanguageSwitcher'

export default function Layout() {
  const { t } = useI18n()
  const state = useSession(s => s.state)
  const refresh = useSession(s => s.refresh)
  const items = [
    { to: '/', icon: Users, label: t('nav.administrators'), end: true },
    { to: '/status', icon: Server, label: t('nav.status') },
  ]
  async function signOut() {
    try { await logout() } catch (e) { toast.error(e?.error || t('common.refused')) }
    await refresh()
  }
  return (
    <div className="flex min-h-dvh">
      <aside className="flex w-60 shrink-0 flex-col border-r border-cream-300 bg-cream-50 px-4 py-6">
        <p className="px-2 text-xs font-semibold tracking-widest text-ink-500">{t('brand')}</p>
        <nav className="mt-6 flex flex-col gap-1">
          {items.map(({ to, icon: Icon, label, end }) => (
            <NavLink key={to} to={to} end={end} className={({ isActive }) => `flex items-center gap-2 rounded-md px-2 py-2 text-sm ${isActive ? 'bg-green-100 font-medium text-green-700' : 'text-ink-700 hover:bg-cream-200'}`}>
              <Icon size={16} />{label}
            </NavLink>
          ))}
        </nav>
        <div className="mt-auto space-y-3 px-2">
          <LanguageSwitcher className="w-full" />
          <p className="text-xs text-ink-500">{t('nav.signedInAs')} <code className="text-ink-900">{state?.login}</code></p>
          <button type="button" onClick={signOut} className="flex items-center gap-2 text-sm text-ink-700 hover:text-terra-600"><LogOut size={15} />{t('nav.signOut')}</button>
        </div>
      </aside>
      <main className="flex-1 overflow-y-auto px-8 py-8"><Outlet /></main>
    </div>
  )
}
