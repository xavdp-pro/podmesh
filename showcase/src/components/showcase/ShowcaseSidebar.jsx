import React, { useEffect, useRef } from 'react'
import {
  Layers,
  HardDrive,
  ShieldCheck,
  Server,
  Terminal,
  PanelLeftClose,
  PanelLeftOpen,
  X,
  ChevronRight,
  Bookmark,
} from 'lucide-react'

const SIDEBAR_ITEMS = [
  {
    key: 'podmesh',
    icon: Layers,
    title: { fr: 'PodMesh Engine', en: 'PodMesh Engine', es: 'PodMesh Engine' },
    desc: {
      fr: 'Architecture multi-fractales & 4 strates',
      en: 'Multi-fractal architecture & 4 strata',
      es: 'Arquitectura multifractal y 4 estratos',
    },
    anchors: [
      { id: 'strata-law', label: { fr: 'Loi des 4 Strates', en: '4 Strata Law', es: 'Ley de 4 Estratos' } },
      { id: 'strata-diagram', label: { fr: 'Schéma Haute Définition', en: 'High-Def Diagram', es: 'Diagrama Alta Definición' } },
      { id: 'strata-isolation', label: { fr: 'Isolation userns & cgroups v2', en: 'userns & cgroups v2', es: 'Aislamiento userns y cgroups' } },
    ],
  },
  {
    key: 'meshfs',
    icon: HardDrive,
    title: { fr: 'MeshFS Distribué', en: 'Distributed MeshFS', es: 'MeshFS Distribuido' },
    desc: {
      fr: 'Stockage CAS 4MB & multi-moteurs',
      en: '4MB CAS storage & multi-engines',
      es: 'Almacenamiento CAS 4MB y multimotores',
    },
    anchors: [
      { id: 'meshfs-merkle', label: { fr: 'Arbre de Merkle & Chunks', en: 'Merkle Tree & Chunks', es: 'Árbol Merkle y Chunks' } },
      { id: 'meshfs-engines', label: { fr: 'Moteurs ZFS / Btrfs / LVM', en: 'ZFS / Btrfs / LVM Engines', es: 'Motores ZFS / Btrfs / LVM' } },
      { id: 'meshfs-ceph-comparison', label: { fr: 'Comparatif vs CephFS', en: 'CephFS Comparison', es: 'Comparativa vs CephFS' } },
    ],
  },
  {
    key: 'pmbs',
    icon: ShieldCheck,
    title: { fr: 'PMBS Backup Server', en: 'PMBS Backup Server', es: 'PMBS Backup Server' },
    desc: {
      fr: 'Datastore dédupliqué & Op 10',
      en: 'Deduplicated datastore & Op 10',
      es: 'Datastore deduplicado y Op 10',
    },
    anchors: [
      { id: 'pmbs-cas', label: { fr: 'Datastore SHA-256 CAS', en: 'SHA-256 CAS Datastore', es: 'Datastore SHA-256 CAS' } },
      { id: 'pmbs-op10', label: { fr: 'Preuve de Survie Op 10', en: 'Op 10 Survival Seal', es: 'Prueba Supervivencia Op 10' } },
      { id: 'pmbs-remotesync', label: { fr: 'Réplication WAN Multi-Sites', en: 'Cross-Site WAN Sync', es: 'Sincronización WAN' } },
    ],
  },
  {
    key: 'proxmox',
    icon: Server,
    title: { fr: 'Plugin Proxmox VE', en: 'Proxmox VE Plugin', es: 'Plugin Proxmox VE' },
    desc: {
      fr: 'Intégration stockage PVE native',
      en: 'Native PVE storage integration',
      es: 'Integración almacenamiento PVE',
    },
    anchors: [
      { id: 'pve-driver', label: { fr: 'Driver Perl PVE::Storage', en: 'Perl Storage Driver', es: 'Driver Perl PVE::Storage' } },
      { id: 'pve-config', label: { fr: 'Configuration storage.cfg', en: 'storage.cfg Configuration', es: 'Configuración storage.cfg' } },
      { id: 'pve-workloads', label: { fr: 'Volumes QEMU, LXC & VZDump', en: 'QEMU, LXC & VZDump', es: 'Volúmenes QEMU, LXC y VZDump' } },
    ],
  },
  {
    key: 'downloads',
    icon: Terminal,
    title: { fr: 'Téléchargements & CLI', en: 'Downloads & CLI', es: 'Descargas y CLI' },
    desc: {
      fr: 'Paquets Debian .deb & Cheatsheet',
      en: 'Debian .deb packages & Cheatsheet',
      es: 'Paquetes Debian .deb y Guía',
    },
    anchors: [
      { id: 'cli-deb', label: { fr: 'Paquets Debian .deb', en: 'Debian .deb Packages', es: 'Paquetes Debian .deb' } },
      { id: 'cli-oci', label: { fr: 'Matrices OCI Déconnectées', en: 'Offline OCI Matrices', es: 'Matrices OCI Desconectadas' } },
      { id: 'cli-terminal', label: { fr: 'Cheatsheet Interactive', en: 'Interactive Cheatsheet', es: 'Terminal Interactivo' } },
    ],
  },
]

const UI_TEXT = {
  fr: {
    summary: 'Sommaire Technique',
    collapse: 'Replier le sommaire',
    expand: 'Déplier le sommaire',
    close: 'Fermer le menu',
  },
  en: {
    summary: 'Technical Contents',
    collapse: 'Collapse sidebar',
    expand: 'Expand sidebar',
    close: 'Close menu',
  },
  es: {
    summary: 'Índice Técnico',
    collapse: 'Plegar índice',
    expand: 'Desplegar índice',
    close: 'Cerrar menú',
  },
}

export default function ShowcaseSidebar({
  activeTab,
  onSelectTab,
  collapsed,
  onToggleCollapsed,
  mobileOpen,
  onCloseMobile,
  locale = 'fr',
}) {
  const drawerRef = useRef(null)
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
            <button
              type="button"
              onClick={() => {
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
                  <p className="truncate text-xs font-bold leading-tight">{title}</p>
                  <p className="truncate text-[10px] text-fg-faint mt-0.5">{desc}</p>
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
            </button>

            {/* Sub-anchors when active */}
            {isActive && (!collapsed || isDrawer) && item.anchors && (
              <div className="ml-8 mt-1 mb-2 flex flex-col gap-1 border-l-2 border-accent/25 pl-3">
                {item.anchors.map((anchor) => (
                  <button
                    key={anchor.id}
                    type="button"
                    onClick={() => {
                      const el = document.getElementById(anchor.id)
                      if (el) {
                        el.scrollIntoView({ behavior: 'smooth', block: 'start' })
                      }
                      if (isDrawer) onCloseMobile()
                    }}
                    className="flex items-center gap-1.5 py-1 text-[11px] font-medium text-fg-muted hover:text-accent transition-colors text-left"
                  >
                    <span className="h-1 w-1 rounded-full bg-accent/60" />
                    <span>{anchor.label[locale] || anchor.label.fr}</span>
                  </button>
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

      {/* Desktop Sticky Retractable Sidebar */}
      <aside
        className={`hidden shrink-0 transition-[width] duration-200 lg:block ${
          collapsed ? 'w-16' : 'w-72'
        }`}
        aria-label={lang.summary}
      >
        <div className="sticky top-[80px] max-h-[calc(100vh-96px)] overflow-y-auto thin-scrollbar rounded-2xl border border-line bg-surface/80 p-2 shadow-xs backdrop-blur-md">
          {/* Header with collapse/expand toggle */}
          <div
            className={`mb-2 flex items-center border-b border-line pb-2 px-2 pt-1 ${
              collapsed ? 'justify-center' : 'justify-between'
            }`}
          >
            {!collapsed && (
              <div className="flex items-center gap-2">
                <Bookmark size={14} className="text-accent" />
                <span className="text-[11px] font-black uppercase tracking-wider text-fg">
                  {lang.summary}
                </span>
              </div>
            )}
            <button
              type="button"
              onClick={onToggleCollapsed}
              className="rounded-lg p-1 text-fg-faint hover:bg-surface-2 hover:text-fg transition-colors"
              title={collapsed ? lang.expand : lang.collapse}
              aria-label={collapsed ? lang.expand : lang.collapse}
            >
              {collapsed ? <PanelLeftOpen size={16} /> : <PanelLeftClose size={16} />}
            </button>
          </div>

          {/* Navigation Items */}
          {renderNavItems(false)}

          {/* GitHub Repositories Link in Sidebar */}
          {!collapsed && (
            <div className="mt-4 border-t border-line pt-3 px-2">
              <span className="text-[10px] font-bold uppercase tracking-wider text-fg-faint block mb-2">
                Code Source &amp; Architecture
              </span>
              <a
                href="https://github.com/xavdp-pro/podmesh"
                target="_blank"
                rel="noopener noreferrer"
                className="flex items-center gap-2 rounded-lg px-2.5 py-1.5 text-xs font-semibold text-fg-muted hover:text-fg hover:bg-surface-2 transition-colors"
              >
                <span className="font-mono text-accent">🐙</span>
                <span>GitHub : podmesh</span>
              </a>
              <a
                href="https://github.com/xavdp-pro/SHAPER-OS-V1.13"
                target="_blank"
                rel="noopener noreferrer"
                className="flex items-center gap-2 rounded-lg px-2.5 py-1.5 text-xs font-semibold text-fg-muted hover:text-fg hover:bg-surface-2 transition-colors mt-0.5"
              >
                <span className="font-mono text-accent">🌿</span>
                <span>GitHub : SHAPER-OS</span>
              </a>
            </div>
          )}
        </div>
      </aside>
    </>
  )
}
