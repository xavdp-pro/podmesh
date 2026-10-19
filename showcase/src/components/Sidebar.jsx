import { useEffect, useRef, useState } from 'react'
import { NavLink } from 'react-router-dom'
import { ChevronDown, ChevronRight, LogOut, PanelLeft, PanelLeftClose, User, X } from 'lucide-react'
import { nav } from '../data/nav'
import { resolveIcon } from './iconMap'
import { useI18n } from '../i18n/I18nProvider'
import { useAuth } from '../auth/AuthProvider'
import LanguageSwitcher from './LanguageSwitcher'
import ThemeToggle from './ui/ThemeToggle'

const STORAGE_GROUPS = 'sidebar-groups'

function loadGroups() {
  const defaults = Object.fromEntries(nav.map((g) => [g.id, true]))
  try {
    return { ...defaults, ...JSON.parse(localStorage.getItem(STORAGE_GROUPS) || '{}') }
  } catch {
    return defaults
  }
}

function NavItem({ to, icon: Icon, label, collapsed, end, onNavigate }) {
  return (
    <NavLink
      to={to}
      end={end}
      onClick={onNavigate}
      className={({ isActive }) =>
        `flex items-center gap-2.5 rounded-md text-sm font-medium transition-colors ${
          collapsed ? 'justify-center px-2 py-2.5' : 'px-3 py-2'
        } ${isActive ? 'bg-accent-soft font-semibold text-accent' : 'text-fg-muted hover:bg-surface-2 hover:text-fg'}`
      }
      title={collapsed ? label : undefined}
    >
      <Icon size={17} className="shrink-0" />
      {!collapsed && <span className="truncate leading-snug">{label}</span>}
    </NavLink>
  )
}

/**
 * Navigation drawer (mobile) / column (desktop), fed by src/data/nav.js.
 * Group open state and the collapsed width survive reloads through localStorage.
 */
export default function Sidebar({ appName, mobileOpen, onMobileClose, onNavigate, collapsed, onToggleCollapsed }) {
  const { t } = useI18n()
  const { user, logout } = useAuth()
  const [groupsOpen, setGroupsOpen] = useState(loadGroups)
  const [userMenuOpen, setUserMenuOpen] = useState(false)
  const userMenuRef = useRef(null)

  useEffect(() => {
    function onDocMouseDown(e) {
      if (userMenuRef.current && !userMenuRef.current.contains(e.target)) setUserMenuOpen(false)
    }
    document.addEventListener('mousedown', onDocMouseDown)
    return () => document.removeEventListener('mousedown', onDocMouseDown)
  }, [])

  function toggleGroup(id) {
    setGroupsOpen((prev) => {
      const next = { ...prev, [id]: !prev[id] }
      try {
        localStorage.setItem(STORAGE_GROUPS, JSON.stringify(next))
      } catch {
        // storage unavailable
      }
      return next
    })
  }

  const iconOnly = collapsed && !mobileOpen

  // Items may be restricted to roles; a group left empty disappears.
  const visibleNav = nav
    .map((group) => ({ ...group, items: group.items.filter((item) => !item.roles || item.roles.includes(user?.role)) }))
    .filter((group) => group.items.length > 0)

  return (
    <aside
      className={[
        'fixed inset-y-0 left-0 z-[60] flex h-[100dvh] flex-col border-r border-line bg-surface transition-[width,transform] duration-200 ease-out',
        'w-[min(18rem,88vw)]',
        mobileOpen ? 'translate-x-0' : '-translate-x-full',
        'lg:static lg:translate-x-0',
        collapsed ? 'lg:w-16' : 'lg:w-64',
      ].join(' ')}
      aria-label="navigation"
    >
      <div className={`flex shrink-0 items-center border-b border-line ${iconOnly ? 'justify-center py-3' : 'h-14 gap-2 px-3'}`}>
        <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-accent text-accent-fg text-xs font-black">
          {appName.slice(0, 1).toUpperCase()}
        </span>
        {!iconOnly && <span className="min-w-0 flex-1 truncate text-base font-black text-fg">{appName}</span>}
        <button type="button" onClick={onMobileClose} className="rounded p-1 text-fg-muted lg:hidden" aria-label={t('sidebar.close')}>
          <X size={20} />
        </button>
        {!iconOnly && (
          <button type="button" onClick={onToggleCollapsed} className="hidden rounded-lg p-1.5 text-fg-faint hover:bg-surface-2 lg:flex" aria-label={t('sidebar.collapse')} title={t('sidebar.collapse')}>
            <PanelLeftClose size={18} />
          </button>
        )}
      </div>

      <nav className="thin-scrollbar flex-1 space-y-1 overflow-y-auto overflow-x-hidden p-2 pb-4">
        {visibleNav.map((group) => (
          <div key={group.id} className="mt-1">
            {!iconOnly && (
              <button
                type="button"
                onClick={() => toggleGroup(group.id)}
                className="flex w-full items-center gap-1.5 rounded-md px-2 py-1.5 text-left text-[11px] font-bold uppercase tracking-wide text-fg-faint hover:bg-surface-2"
              >
                {groupsOpen[group.id] ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
                <span className="truncate">{t(group.label)}</span>
              </button>
            )}
            {(iconOnly || groupsOpen[group.id]) &&
              group.items.map((item) => (
                <NavItem key={item.to} to={item.to} icon={resolveIcon(item.icon)} label={t(item.label)} collapsed={iconOnly} end={item.end} onNavigate={onNavigate} />
              ))}
          </div>
        ))}
      </nav>

      <div className="shrink-0 space-y-1 border-t border-line p-2">
        {iconOnly ? (
          <button type="button" onClick={onToggleCollapsed} className="flex w-full items-center justify-center rounded-md py-2.5 text-fg-muted hover:bg-surface-2" aria-label={t('sidebar.expand')} title={t('sidebar.expand')}>
            <PanelLeft size={18} />
          </button>
        ) : (
          <div className="flex items-center gap-2 px-1 pb-1">
            <ThemeToggle />
            <LanguageSwitcher />
          </div>
        )}

        <div ref={userMenuRef} className="relative">
          <button
            type="button"
            onClick={() => setUserMenuOpen((v) => !v)}
            className={`flex w-full items-center rounded-xl text-sm transition-colors hover:bg-surface-2 ${iconOnly ? 'justify-center p-2' : 'gap-2.5 px-2.5 py-2'}`}
            aria-label={t('sidebar.session')}
            aria-expanded={userMenuOpen}
          >
            <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-accent text-xs font-bold text-accent-fg">
              {user?.name ? user.name.charAt(0).toUpperCase() : <User size={15} />}
            </div>
            {!iconOnly && (
              <div className="min-w-0 flex-1 text-left">
                <p className="truncate text-xs font-semibold leading-tight text-fg">{user?.name || t('sidebar.session')}</p>
                <p className="truncate text-[10px] leading-tight text-fg-faint">{user?.email}</p>
              </div>
            )}
          </button>
          {userMenuOpen && (
            <div className="absolute bottom-full left-0 z-50 mb-1.5 w-52 rounded-xl border border-line bg-surface p-2 shadow-xl">
              <div className="mb-1 border-b border-line px-3 py-2">
                <p className="text-xs font-bold text-fg">{user?.name}</p>
                <p className="truncate text-[11px] text-fg-faint">{user?.email}</p>
              </div>
              <button type="button" onClick={logout} className="flex w-full items-center gap-2 rounded-lg px-3 py-2 text-left text-xs font-medium text-danger hover:bg-danger-soft">
                <LogOut size={14} /> {t('sidebar.logout')}
              </button>
            </div>
          )}
        </div>
      </div>
    </aside>
  )
}
