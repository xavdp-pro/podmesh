import { useCallback, useEffect, useRef, useState } from 'react'
import { Outlet, useLocation } from 'react-router-dom'
import { ArrowUp, Menu } from 'lucide-react'
import Sidebar from './Sidebar'
import { ToastHost } from './ui/ToastHost'
import { useI18n } from '../i18n/I18nProvider'

const STORAGE_COLLAPSED = 'sidebar-collapsed'
const BACK_TO_TOP_AFTER = 600

/**
 * App shell: sidebar + scrolling <main>. The window never scrolls; <main> does,
 * so scroll-to-top on route change and the back-to-top button live here.
 * Pages that render <pre> inside a grid must keep `min-w-0` on the grid cell.
 */
export default function Layout({ appName }) {
  const { t } = useI18n()
  const { pathname } = useLocation()
  const mainRef = useRef(null)
  const [mobileNavOpen, setMobileNavOpen] = useState(false)
  const [collapsed, setCollapsed] = useState(() => {
    try {
      return localStorage.getItem(STORAGE_COLLAPSED) === 'true'
    } catch {
      return false
    }
  })
  const [showBackToTop, setShowBackToTop] = useState(false)

  const closeMobileNav = useCallback(() => setMobileNavOpen(false), [])
  const scrollToTop = useCallback((smooth = false) => {
    mainRef.current?.scrollTo({ top: 0, behavior: smooth ? 'smooth' : 'auto' })
  }, [])

  useEffect(() => {
    scrollToTop()
    setMobileNavOpen(false)
  }, [pathname, scrollToTop])

  useEffect(() => {
    const el = mainRef.current
    if (!el) return undefined
    const onScroll = () => setShowBackToTop(el.scrollTop > BACK_TO_TOP_AFTER)
    el.addEventListener('scroll', onScroll, { passive: true })
    return () => el.removeEventListener('scroll', onScroll)
  }, [])

  function toggleCollapsed() {
    const next = !collapsed
    setCollapsed(next)
    try {
      localStorage.setItem(STORAGE_COLLAPSED, String(next))
    } catch {
      // storage unavailable
    }
  }

  return (
    <div className="flex h-[100dvh] min-w-0 overflow-hidden bg-canvas">
      {mobileNavOpen && (
        <button type="button" className="fixed inset-0 z-[55] bg-black/40 lg:hidden" aria-label={t('sidebar.close')} onClick={closeMobileNav} />
      )}
      <Sidebar
        appName={appName}
        mobileOpen={mobileNavOpen}
        onMobileClose={closeMobileNav}
        onNavigate={() => {
          closeMobileNav()
          scrollToTop()
        }}
        collapsed={collapsed}
        onToggleCollapsed={toggleCollapsed}
      />
      <div className="relative flex min-w-0 flex-1 flex-col overflow-hidden">
        <header className="flex h-12 shrink-0 items-center gap-3 border-b border-line bg-surface/95 px-3 backdrop-blur-sm lg:hidden">
          <button type="button" onClick={() => setMobileNavOpen(true)} className="flex h-10 w-10 items-center justify-center rounded-lg text-fg-muted hover:bg-surface-2" aria-label={t('sidebar.open')}>
            <Menu size={22} />
          </button>
          <span className="truncate text-sm font-black text-fg">{appName}</span>
        </header>

        <main ref={mainRef} className="min-h-0 min-w-0 flex-1 overflow-x-hidden overflow-y-auto">
          <div className="mx-auto w-full max-w-6xl min-w-0 p-4 sm:p-6 lg:p-8">
            <Outlet />
          </div>
        </main>

        {showBackToTop && (
          <button
            type="button"
            onClick={() => scrollToTop(true)}
            className="absolute bottom-5 right-5 z-[45] flex h-11 w-11 items-center justify-center rounded-full bg-accent text-accent-fg shadow-lg transition-colors hover:bg-accent-hover"
            aria-label={t('sidebar.backToTop')}
            title={t('sidebar.backToTop')}
          >
            <ArrowUp size={20} />
          </button>
        )}
      </div>
      <ToastHost />
    </div>
  )
}
