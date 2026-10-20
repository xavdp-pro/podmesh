import React, { useEffect, useRef } from 'react'
import { guideContent } from '../../data/publicGuideContent'
import {
  Layers,
  HardDrive,
  ShieldCheck,
  Server,
  Terminal,
  X,
  ChevronRight,
  Bookmark,
} from 'lucide-react'

const NAV_ICONS = { podmesh: Layers, meshfs: HardDrive, pmbs: ShieldCheck, proxmox: Server, downloads: Terminal }
const LOCALES = ['fr', 'en', 'es']
const translated = (read) => Object.fromEntries(LOCALES.map((locale) => [locale, read(guideContent[locale])]))

export const SIDEBAR_ITEMS = guideContent.fr.sections.map(({ id }, index) => ({
  key: id,
  icon: NAV_ICONS[id],
  title: translated((copy) => copy.sections[index].name),
  desc: translated((copy) => copy.sections[index].short),
  anchors: ['usage', 'agent', 'details'].map((part) => ({
    id: `${id}-${part}`,
    label: translated((copy) => copy[part]),
  })),
}))

const UI_TEXT = {
  fr: {
    summary: 'Explorer les outils',
    collapse: 'Replier le sommaire',
    expand: 'Déplier le sommaire',
    close: 'Fermer le menu',
  },
  en: {
    summary: 'Explore the tools',
    collapse: 'Collapse sidebar',
    expand: 'Expand sidebar',
    close: 'Close menu',
  },
  es: {
    summary: 'Explorar las herramientas',
    collapse: 'Plegar índice',
    expand: 'Desplegar índice',
    close: 'Cerrar menú',
  },
}

export default function ShowcaseSidebar({
  activeTab,
  onSelectTab,
  desktopExpanded,
  onCollapseDesktop,
  mobileOpen,
  onCloseMobile,
  locale = 'fr',
}) {
  const drawerRef = useRef(null)
  const collapsed = false
  const lang = UI_TEXT[locale] || UI_TEXT.fr

  // Close mobile drawer on Escape key
  useEffect(() => {
    function handleKeyDown(e) {
      if (e.key === 'Escape' && mobileOpen) {
        onCloseMobile()
      }
    }
    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [mobileOpen, onCloseMobile])

  const renderNavItems = (isDrawer = false) => (
    <nav className="flex flex-col gap-1.5 p-2">
      {SIDEBAR_ITEMS.map((item) => {
        const Icon = item.icon
        const isActive = activeTab === item.key
        const title = item.title[locale] || item.title.fr
        const desc = item.desc[locale] || item.desc.fr

        return (
          <div key={item.key} className="flex flex-col">
            <a
              href={`#${item.key}`}
              aria-current={isActive ? 'page' : undefined}
              onClick={(event) => {
                event.preventDefault()
                onSelectTab(item.key)
                if (isDrawer) onCloseMobile()
              }}
              title={collapsed && !isDrawer ? title : undefined}
              className={`group relative flex items-center gap-3 rounded-xl px-3 py-2.5 text-left transition-all ${
                isActive
                  ? 'bg-accent/15 text-accent shadow-xs'
                  : 'text-fg-muted hover:bg-surface-2 hover:text-fg'
              }`}
            >
              {/* Left active glowing indicator bar */}
              {isActive && (
                <span
                  className="absolute left-0 top-1/2 h-5 w-1 -translate-y-1/2 rounded-r-full bg-accent"
                  aria-hidden="true"
                />
              )}

              <div
                className={`flex h-8 w-8 shrink-0 items-center justify-center rounded-lg transition-colors ${
                  isActive
                    ? 'bg-accent text-accent-fg'
                    : 'bg-surface-2 text-fg-muted group-hover:bg-accent/10 group-hover:text-accent'
                }`}
              >
                <Icon size={17} />
              </div>

              {(!collapsed || isDrawer) && (
                <div className="min-w-0 flex-1">
                  <p className="text-sm font-bold leading-tight">{title}</p>
                  <p className="text-xs text-fg-faint mt-0.5">{desc}</p>
                </div>
              )}

              {(!collapsed || isDrawer) && (
                <ChevronRight
                  size={14}
                  className={`shrink-0 transition-transform ${
                    isActive ? 'text-accent translate-x-0.5' : 'text-fg-faint opacity-40 group-hover:opacity-100'
                  }`}
                />
              )}
            </a>

            {/* Sub-anchors when active */}
            {isActive && (!collapsed || isDrawer) && item.anchors && (
              <div className="ml-8 mt-1 mb-2 flex flex-col gap-1 border-l-2 border-accent/25 pl-3">
                {item.anchors.map((anchor) => (
                  <a
                    key={anchor.id}
                    href={`#${anchor.id}`}
                    onClick={(event) => {
                      event.preventDefault()
                      onSelectTab(item.key, anchor.id)
                      if (isDrawer) onCloseMobile()
                    }}
                    className="flex items-center gap-1.5 py-1.5 text-xs font-medium text-fg-muted hover:text-accent transition-colors text-left"
                  >
                    <span className="h-1 w-1 rounded-full bg-accent/60" />
                    <span>{anchor.label[locale] || anchor.label.fr}</span>
                  </a>
                ))}
              </div>
            )}
          </div>
        )
      })}
    </nav>
  )

  return (
    <>
      {/* Mobile Backdrop & Drawer */}
      {mobileOpen && (
        <div className="fixed inset-0 z-50 lg:hidden">
          <div
            className="fixed inset-0 bg-black/60 backdrop-blur-xs transition-opacity"
            onClick={onCloseMobile}
            aria-hidden="true"
          />
          <aside
            ref={drawerRef}
            id="guide-navigation"
            data-mobile-guide
            role="dialog"
            aria-modal="true"
            className="fixed inset-y-0 left-0 z-50 flex w-72 max-w-[85vw] flex-col border-r border-line bg-surface p-3 shadow-2xl"
            aria-label={lang.summary}
          >
            <div className="flex items-center justify-between border-b border-line px-2 pb-3 pt-1">
              <div className="flex items-center gap-2">
                <Bookmark size={16} className="text-accent" />
                <span className="text-xs font-black uppercase tracking-wider text-fg">
                  {lang.summary}
                </span>
              </div>
              <button
                type="button"
                onClick={onCloseMobile}
                className="rounded-lg p-1.5 text-fg-muted hover:bg-surface-2 hover:text-fg"
                aria-label={lang.close}
              >
                <X size={18} />
              </button>
            </div>
            <div className="flex-1 overflow-y-auto thin-scrollbar pt-2">
              {renderNavItems(true)}
            </div>
          </aside>
        </div>
      )}

      {desktopExpanded && (
        <aside id="guide-navigation-desktop" aria-label={lang.summary}
          className="sticky top-20 hidden w-72 shrink-0 self-start lg:block">
          <div className="max-h-[calc(100dvh-96px)] overflow-y-auto rounded-2xl border border-line bg-surface p-3 shadow-xs thin-scrollbar">
            <div className="flex items-center justify-between border-b border-line px-2 pb-3 pt-1">
              <div className="flex items-center gap-2">
                <Bookmark size={16} className="text-accent" />
                <span className="text-xs font-black uppercase tracking-wider">{lang.summary}</span>
              </div>
              <button type="button" onClick={onCollapseDesktop} aria-label={lang.collapse}
                className="rounded-lg p-1.5 text-fg-muted hover:bg-surface-2 hover:text-fg">
                <X size={18} />
              </button>
            </div>
            <div className="pt-2">{renderNavItems(false)}</div>
          </div>
        </aside>
      )}


    </>
  )
}
