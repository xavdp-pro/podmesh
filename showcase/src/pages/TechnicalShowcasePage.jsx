import { useState } from 'react'
import { Link } from 'react-router-dom'
import { motion, AnimatePresence } from 'framer-motion'
import {
  Activity,
  ArrowRight,
  ArrowRightLeft,
  Boxes,
  Check,
  CheckCircle2,
  Clock,
  Code2,
  Copy,
  Cpu,
  Database,
  Download,
  ExternalLink,
  Flame,
  FolderTree,
  HardDrive,
  Layers,
  Network,
  Play,
  RefreshCw,
  Server,
  Shield,
  ShieldCheck,
  Sparkles,
  Terminal,
  Menu,
  PanelLeftClose,
  PanelLeftOpen,
  ChevronLeft,
  ChevronRight,
  Zap,
} from 'lucide-react'
import { useI18n } from '../i18n/I18nProvider'
import LanguageSwitcher from '../components/LanguageSwitcher'
import ThemeToggle from '../components/ui/ThemeToggle'
import ShowcaseSidebar from '../components/showcase/ShowcaseSidebar'
import PerspectiveTandemSection, { TechnologyPerspectiveCards, GitHubIcon } from '../components/showcase/PerspectiveTandemSection'
import { showcaseContent, cliSnippets } from '../data/showcaseContent'
import { StrataSvgDiagram, MeshFsSvgDiagram, FederationDualLabSvgDiagram } from '../components/showcase/ShaperSvgDiagrams'

const TAB_KEYS = ['podmesh', 'meshfs', 'pmbs', 'proxmox', 'downloads']

export default function TechnicalShowcasePage() {
  const { locale, setLocale, t } = useI18n()
  const user = null
  const [activeTab, setActiveTab] = useState('podmesh')
  const [activeStrate, setActiveStrate] = useState('0')
  const [op10Step, setOp10Step] = useState(0)
  const [copiedText, setCopiedText] = useState(null)
  const [cliCategory, setCliCategory] = useState('podmesh')
  const [currentPerspective, setCurrentPerspective] = useState('all')

  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => {
    try {
      return localStorage.getItem('showcase:sidebar:collapsed') === 'true'
    } catch {
      return false
    }
  })
  const [mobileDrawerOpen, setMobileDrawerOpen] = useState(false)

  const toggleSidebar = () => {
    setSidebarCollapsed((prev) => {
      const next = !prev
      try {
        localStorage.setItem('showcase:sidebar:collapsed', String(next))
      } catch {}
      return next
    })
  }

  const content = showcaseContent[locale] || showcaseContent.fr

  function copyToClipboard(text, id) {
    if (navigator?.clipboard?.writeText) {
      navigator.clipboard.writeText(text).catch(() => {})
    }
    setCopiedText(id)
    setTimeout(() => setCopiedText(null), 2500)
  }

  const currentIndex = TAB_KEYS.indexOf(activeTab)
  const prevTab = currentIndex > 0 ? TAB_KEYS[currentIndex - 1] : null
  const nextTab = currentIndex < TAB_KEYS.length - 1 ? TAB_KEYS[currentIndex + 1] : null

  return (
    <div className="min-h-screen bg-canvas text-fg transition-colors duration-200">
      {/* Top Floating Bar / Navbar */}
      <header className="sticky top-0 z-40 border-b border-line bg-surface/90 backdrop-blur-md">
        <div className="mx-auto flex h-16 max-w-7xl items-center justify-between px-4 sm:px-6 lg:px-8">
          <div className="flex items-center gap-3">
            {/* Mobile Drawer Trigger */}
            <button
              type="button"
              onClick={() => setMobileDrawerOpen(true)}
              className="flex h-9 w-9 items-center justify-center rounded-xl border border-line bg-surface text-fg-muted hover:text-fg hover:bg-surface-2 lg:hidden shadow-xs"
              aria-label="Ouvrir le sommaire"
            >
              <Menu size={18} />
            </button>

            {/* Desktop Sidebar Toggle */}
            <button
              type="button"
              onClick={toggleSidebar}
              className="hidden lg:flex h-9 w-9 items-center justify-center rounded-xl border border-line bg-surface text-fg-muted hover:text-fg hover:bg-surface-2 transition-colors shadow-xs"
              title={sidebarCollapsed ? 'Déplier le sommaire' : 'Replier le sommaire'}
              aria-label={sidebarCollapsed ? 'Déplier le sommaire' : 'Replier le sommaire'}
            >
              {sidebarCollapsed ? <PanelLeftOpen size={18} /> : <PanelLeftClose size={18} />}
            </button>

            {/* Brand Lockup (Atelier Vivant) */}
            <Link to="/" className="group flex items-center gap-3 shrink-0 select-none py-1 focus:outline-none">
              <span className="site-display text-2xl font-black tracking-tight text-fg leading-none transition-transform duration-200 group-hover:scale-105">
                PodMesh
              </span>
              <div className="h-6 w-[1.5px] bg-line opacity-80 transition-colors duration-200 group-hover:bg-accent" aria-hidden="true" />
              <div className="flex flex-col justify-center text-[9px] font-bold uppercase tracking-[0.14em] leading-tight">
                <span className="whitespace-nowrap text-fg-muted transition-colors group-hover:text-fg">
                  Distributed Cloud OS
                </span>
                <span className="whitespace-nowrap text-accent transition-colors group-hover:text-accent-hover">
                  MeshFS &amp; PMBS Engine
                </span>
              </div>
            </Link>
          </div>

          {/* Active Chapter Breadcrumb Badge */}
          <div className="hidden md:flex items-center gap-2 rounded-full border border-line bg-surface-2/80 px-3.5 py-1 text-xs font-semibold shadow-xs">
            <span className="h-2 w-2 rounded-full bg-accent animate-pulse" />
            <span className="text-fg-muted font-normal">Chapitre :</span>
            <span className="text-fg font-bold">{content.navTabs[activeTab]}</span>
          </div>

          <div className="flex items-center gap-2.5">
            <a
              href="https://github.com/xavdp-pro/podmesh"
              target="_blank"
              rel="noopener noreferrer"
              className="flex items-center gap-1.5 rounded-full border border-line bg-surface/80 px-3 py-1.5 text-xs font-bold text-fg hover:bg-surface-2 transition-colors shadow-xs"
              title="Code source PodMesh sur GitHub"
            >
              <GitHubIcon size={15} />
              <span className="hidden sm:inline">GitHub</span>
            </a>
            <LanguageSwitcher />
            <ThemeToggle />
          </div>
        </div>
      </header>

      {/* Main Container with Sidebar + Content (Sidebar starts at top, never blocked by hero) */}
      <div className="mx-auto flex w-full max-w-7xl items-start gap-6 lg:gap-8 px-4 py-6 sm:px-6 lg:px-8 flex-1">
        <ShowcaseSidebar
          activeTab={activeTab}
          onSelectTab={(tabKey) => {
            setActiveTab(tabKey)
            window.scrollTo({ top: 0, behavior: 'smooth' })
          }}
          collapsed={sidebarCollapsed}
          onToggleCollapsed={toggleSidebar}
          mobileOpen={mobileDrawerOpen}
          onCloseMobile={() => setMobileDrawerOpen(false)}
          locale={locale}
        />

        <main className="min-w-0 flex-1 space-y-8">
          {/* Bannière Doctrinale Shaper OS : Tandem Humain ⇄ Agent IA & Sélecteur de Niveau */}
          <PerspectiveTandemSection
            tandemData={content.tandem}
            githubData={content.github}
            currentPerspective={currentPerspective}
            onSelectPerspective={setCurrentPerspective}
            locale={locale}
          />

          <AnimatePresence mode="wait">
          {/* TAB 1: PODMESH ENGINE */}
          {activeTab === 'podmesh' && (
            <motion.div
              key="podmesh"
              initial={{ opacity: 0, y: 10 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -10 }}
              transition={{ duration: 0.2 }}
              className="space-y-10"
            >
              {/* Grand Hero Box & Telemetry Cards */}
              <div className="rounded-3xl border border-line bg-surface/80 p-6 sm:p-10 shadow-xs space-y-6">
                <div className="flex flex-wrap items-center gap-2">
                  <div className="inline-flex items-center gap-2 rounded-full border border-accent/30 bg-accent/10 px-3.5 py-1 text-xs font-bold text-accent shadow-xs">
                    <Sparkles size={14} className="text-accent" />
                    <span>{content.hero.badge}</span>
                  </div>
                  <span className="rounded-full bg-emerald-500/15 px-3 py-1 text-xs font-bold text-emerald-500">
                    Active Arbiter &lt; 50ms
                  </span>
                </div>

                <div>
                  <h1 className="site-display text-3xl font-black tracking-tight text-fg sm:text-5xl">
                    {content.hero.title}
                  </h1>
                  <p className="mt-3 text-sm leading-relaxed text-fg-muted sm:text-base font-normal max-w-3xl">
                    {content.hero.subtitle}
                  </p>
                </div>

                {/* 4 Telemetry Metrics (Agnostic & General) */}
                <div className="grid grid-cols-2 gap-3 sm:grid-cols-4 sm:gap-4 pt-2">
                  {content.metrics.map((metric, idx) => (
                    <motion.div
                      key={metric.label}
                      initial={{ opacity: 0, scale: 0.95 }}
                      animate={{ opacity: 1, scale: 1 }}
                      transition={{ delay: 0.1 + idx * 0.05 }}
                      className="rounded-2xl border border-line bg-canvas p-4 text-center shadow-xs"
                    >
                      <p className="text-[11px] font-bold uppercase tracking-wider text-fg-faint">{metric.label}</p>
                      <p className="mt-1 text-2xl font-black text-accent sm:text-3xl">{metric.value}</p>
                      <p className="mt-1 text-[11px] font-medium text-fg-muted">{metric.sub}</p>
                    </motion.div>
                  ))}
                </div>
              </div>

              {/* 3 Perspectives : Humain (Non-Geek) / Geek (Kernel) / Agent IA (Machine) */}
              <TechnologyPerspectiveCards
                perspectives={content.podmesh?.perspectives}
                currentPerspective={currentPerspective}
              />

              {/* 4 Strates Interactive Explorer */}
              <section className="space-y-4">
                <div className="flex flex-col justify-between gap-2 sm:flex-row sm:items-center">
                  <div>
                    <h3 className="text-xl font-black text-fg flex items-center gap-2">
                      <Layers className="text-accent" size={22} />
                      {content.podmesh.strataTitle}
                    </h3>
                    <p className="text-xs text-fg-muted mt-1">{content.podmesh.strataDesc}</p>
                  </div>
                </div>

                {/* SVG Visual Schema SHAPER OS v1.15 */}
                <StrataSvgDiagram activeStrate={activeStrate} onSelectStrate={setActiveStrate} />

                {/* Strate Tabs */}
                <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
                  {content.podmesh.strata.map((st) => (
                    <button
                      key={st.num}
                      type="button"
                      onClick={() => setActiveStrate(st.num)}
                      className={`rounded-2xl border p-4 text-left transition-all ${
                        activeStrate === st.num
                          ? 'border-accent bg-accent/10 shadow-sm ring-1 ring-accent'
                          : 'border-line bg-surface hover:border-line-strong'
                      }`}
                    >
                      <div className="flex items-center justify-between">
                        <span className="rounded-lg bg-accent px-2 py-0.5 text-xs font-black text-accent-fg">
                          Strate {st.num}
                        </span>
                        {activeStrate === st.num && <CheckCircle2 size={16} className="text-accent" />}
                      </div>
                      <p className="mt-2 text-xs font-bold text-fg line-clamp-1">{st.name.split('—')[1] || st.name}</p>
                      <p className="mt-1 text-[11px] text-fg-muted truncate">{st.actor}</p>
                    </button>
                  ))}
                </div>

                {/* Active Strate Deep Dive Card */}
                {(() => {
                  const curr = content.podmesh.strata.find((s) => s.num === activeStrate) || content.podmesh.strata[0]
                  return (
                    <div className="rounded-2xl border border-line bg-surface p-6 shadow-xs">
                      <div className="flex flex-wrap items-center justify-between gap-2 border-b border-line pb-4">
                        <div className="flex items-center gap-2.5">
                          <span className="flex h-8 w-8 items-center justify-center rounded-xl bg-accent text-accent-fg text-sm font-black">
                            {curr.num}
                          </span>
                          <div>
                            <h4 className="text-base font-bold text-fg">{curr.name}</h4>
                            <p className="text-xs text-accent font-semibold">{curr.actor}</p>
                          </div>
                        </div>
                        <span className="rounded-md bg-surface-2 px-2.5 py-1 font-mono text-xs text-fg-muted">
                          {curr.location}
                        </span>
                      </div>

                      <div className="mt-5 grid gap-4 md:grid-cols-3">
                        <div className="rounded-xl border border-line bg-surface-2/40 p-4">
                          <p className="text-[11px] font-bold uppercase tracking-wider text-fg-faint">
                            Pouvoir Juridique
                          </p>
                          <p className="mt-1.5 text-xs text-fg leading-relaxed">{curr.power}</p>
                        </div>
                        <div className="rounded-xl border border-line bg-surface-2/40 p-4">
                          <p className="text-[11px] font-bold uppercase tracking-wider text-fg-faint">
                            Politique Réseau
                          </p>
                          <p className="mt-1.5 text-xs text-fg leading-relaxed">{curr.network}</p>
                        </div>
                        <div className="rounded-xl border border-line bg-surface-2/40 p-4">
                          <p className="text-[11px] font-bold uppercase tracking-wider text-fg-faint">
                            Invariant Inviolable
                          </p>
                          <p className="mt-1.5 text-xs text-emerald-600 dark:text-emerald-400 font-medium leading-relaxed">
                            {curr.invariant}
                          </p>
                        </div>
                      </div>

                      {/* Code Sample / Protocol Snippet */}
                      <div className="mt-5 rounded-xl border border-line bg-canvas p-4 font-mono text-xs text-fg">
                        <div className="flex items-center justify-between text-[11px] text-fg-faint pb-2 border-b border-line">
                          <span>{activeStrate === '1' ? 'POST /api/makers/poll Protocol' : activeStrate === '2' ? 'Recette Maker scellée' : 'Runtime cgroups v2 & userns'}</span>
                          <span className="text-emerald-500 font-semibold">Strict SHA-256 Digest ✓</span>
                        </div>
                        <pre className="mt-3 overflow-x-auto text-[11px] leading-relaxed text-fg-muted">
                          {activeStrate === '1'
                            ? `// Gouverneur de Strate 1 : Réponse canonique de travail
{
  "ok": true,
  "work": [
    {
      "workId": "stamp:row-a1b2c3d4",
      "kind": "stamp",
      "rowId": "row-a1b2c3d4",
      "klass": "boutique_woocommerce",
      "matrix": "univ-shaper-core/base-debian13:443dedd",
      "digest": "sha256:958e0b74a1c3f8b2d4e6a7c8b9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8",
      "params": { "client_slug": "boulangerie_dupont", "quota_ram_mb": 512 }
    }
  ],
  "withheld": []
}`
                            : activeStrate === '2'
                            ? `#!/usr/bin/env bash
# Maker Strate 2 : Recette stamp.sh scellée
set -euo pipefail
ROW_ID="$1"; CLASS="$2"; MATRIX="$3"; DIGEST="$4"
# Vérification d'intégrité avant tout travail :
REAL_DIGEST=$(sha256sum "/var/lib/podmesh/matrices/\${MATRIX}.tar" | awk '{print $1}')
[[ "sha256:\${REAL_DIGEST}" == "\${DIGEST}" ]] || exit 3
podman create --userns=auto --cgroups=split --memory=\${SHAPER_PARAM_QUOTA_RAM_MB}m "\${MATRIX}"`
                            : activeStrate === '0'
                            ? `# Strate 0 Sovereign Manager : Relocalisation à chaud 42ms
btrfs subvolume snapshot -r /var/lib/podmesh/universes/univ-demo /snapshots/delta-01
btrfs send -p /snapshots/base /snapshots/delta-01 | ssh worker-replica-host btrfs receive /universes/
podman container checkpoint univ-demo --leave-running=false
# Bascule Quorum Ed25519 epoch 42 -> Service Purring`
                            : `// Strate 3 Client Sandbox
uid_map: 0 100000 65536
gid_map: 0 100000 65536
cgroups.controllers: cpu memory io pids
network: veth-podmesh-prod <-> br-client-isolated`}
                        </pre>
                      </div>
                    </div>
                  )
                })()}
              </section>

              {/* Core Features Cards */}
              <div className="grid gap-4 md:grid-cols-3">
                {content.podmesh.features.map((feat) => (
                  <div key={feat.title} className="rounded-2xl border border-line bg-surface p-5 shadow-xs">
                    <div className="flex h-9 w-9 items-center justify-center rounded-xl bg-accent/10 text-accent">
                      <ShieldCheck size={20} />
                    </div>
                    <h4 className="mt-3 text-base font-bold text-fg">{feat.title}</h4>
                    <p className="mt-2 text-xs leading-relaxed text-fg-muted">{feat.desc}</p>
                  </div>
                ))}
              </div>
            </motion.div>
          )}

          {/* TAB 2: PODMESH BACKUP SERVER (PMBS) */}
          {activeTab === 'pmbs' && (
            <motion.div
              key="pmbs"
              initial={{ opacity: 0, y: 10 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -10 }}
              transition={{ duration: 0.2 }}
              className="space-y-10"
            >
              <div className="rounded-3xl border border-line bg-surface p-6 sm:p-8 shadow-xs">
                <span className="rounded-full bg-emerald-500/15 px-3 py-1 text-xs font-bold text-emerald-500">
                  {content.pmbs.tag}
                </span>
                <h2 className="mt-3 text-2xl font-black text-fg sm:text-3xl">{content.pmbs.title}</h2>
                <p className="mt-2 text-sm leading-relaxed text-fg-muted sm:text-base">{content.pmbs.lead}</p>
              </div>

              {/* 3 Perspectives : Humain (Non-Geek) / Geek (Kernel) / Agent IA (Machine) */}
              <TechnologyPerspectiveCards
                perspectives={content.pmbs?.perspectives}
                currentPerspective={currentPerspective}
              />

              {/* SVG Topologie Fédérée Multi-Sites & PMBS Sync */}
              <FederationDualLabSvgDiagram />

              {/* Op 10 Sandbox Boot Restore Interactive Wizard */}
              <section className="rounded-2xl border border-line bg-surface p-6 shadow-xs">
                <div className="flex flex-col justify-between gap-3 sm:flex-row sm:items-center">
                  <div>
                    <h3 className="text-xl font-black text-fg flex items-center gap-2">
                      <Shield className="text-emerald-500" size={22} />
                      {content.pmbs.op10Title}
                    </h3>
                    <p className="text-xs text-fg-muted mt-1">{content.pmbs.op10Desc}</p>
                  </div>
                  <button
                    type="button"
                    onClick={() => setOp10Step((prev) => (prev + 1) % 4)}
                    className="flex items-center gap-1.5 rounded-xl bg-emerald-600 px-3.5 py-2 text-xs font-bold text-white shadow-sm hover:bg-emerald-500"
                  >
                    <Play size={14} /> Simuler Étape Suivante ({op10Step + 1}/4)
                  </button>
                </div>

                <div className="mt-6 grid gap-3 sm:grid-cols-4">
                  {content.pmbs.op10Steps.map((s, idx) => {
                    const isPassed = op10Step >= idx
                    const isCurrent = op10Step === idx
                    return (
                      <div
                        key={s.step}
                        onClick={() => setOp10Step(idx)}
                        className={`cursor-pointer rounded-xl border p-4 transition-all ${
                          isCurrent
                            ? 'border-emerald-500 bg-emerald-500/10 ring-1 ring-emerald-500'
                            : isPassed
                            ? 'border-emerald-500/40 bg-surface'
                            : 'border-line bg-surface-2/40 opacity-60'
                        }`}
                      >
                        <div className="flex items-center justify-between">
                          <span
                            className={`rounded-md px-2 py-0.5 text-xs font-black font-mono ${
                              isCurrent ? 'bg-emerald-500 text-white' : 'bg-surface-2 text-fg-muted'
                            }`}
                          >
                            Étape {s.step}
                          </span>
                          {isPassed && <CheckCircle2 size={16} className="text-emerald-500" />}
                        </div>
                        <h4 className="mt-2 text-sm font-bold text-fg">{s.title}</h4>
                        <p className="mt-1 text-xs text-fg-muted leading-relaxed">{s.text}</p>
                      </div>
                    )
                  })}
                </div>

                <div className="mt-4 rounded-xl border border-emerald-500/30 bg-emerald-500/5 p-4 text-xs font-mono text-fg flex items-center justify-between">
                  <span className="flex items-center gap-2">
                    <span className="h-2 w-2 rounded-full bg-emerald-500 animate-ping" />
                    Op 10 Sandbox Container: <span className="text-emerald-600 dark:text-emerald-400 font-bold">pmbs-sandbox-boot-val-3199</span> (Duration: 1.1s • Health: PURRING)
                  </span>
                  <span className="text-[11px] text-fg-muted">Signature Ed25519 Certifiée</span>
                </div>
              </section>

              {/* SHA-256 CAS & Remote Sync Dual Column */}
              <div className="grid gap-6 md:grid-cols-2">
                <div className="rounded-2xl border border-line bg-surface p-6 shadow-xs space-y-3">
                  <div className="flex items-center gap-2">
                    <Database className="text-accent" size={20} />
                    <h3 className="text-lg font-bold text-fg">{content.pmbs.sha256Title}</h3>
                  </div>
                  <p className="text-xs leading-relaxed text-fg-muted">{content.pmbs.sha256Desc}</p>
                  <div className="rounded-xl border border-line bg-surface-2 p-3 font-mono text-[11px] text-fg-muted">
                    Datastore Root: <span className="text-fg font-bold">/var/lib/pmbs/datastores/ds-prod-01</span>
                    <br />
                    Chunking: <span className="text-accent font-semibold">Content-Defined 4MB Fixed-CAS</span>
                    <br />
                    Taux Déduplication : <span className="text-emerald-500 font-bold">8.4x (88% économie stockage)</span>
                  </div>
                </div>

                <div className="rounded-2xl border border-line bg-surface p-6 shadow-xs space-y-3">
                  <div className="flex items-center gap-2">
                    <ArrowRightLeft className="text-sky-500" size={20} />
                    <h3 className="text-lg font-bold text-fg">{content.pmbs.remoteSyncTitle}</h3>
                  </div>
                  <p className="text-xs leading-relaxed text-fg-muted">{content.pmbs.remoteSyncDesc}</p>
                  <div className="rounded-xl border border-line bg-surface-2 p-3 font-mono text-[11px] text-fg-muted">
                    Flux Inter-Sites: <span className="text-fg font-bold">Datacenter Alpha (Région A) ⇄ Datacenter Beta (Région B)</span>
                    <br />
                    Chiffrement: <span className="text-sky-500 font-semibold">ChaCha20-Poly1305 over WireGuard</span>
                    <br />
                    RPO: <span className="text-emerald-500 font-bold">&lt; 30 secondes</span> • RTO: <span className="text-emerald-500 font-bold">&lt; 2 minutes</span>
                  </div>
                </div>
              </div>

              {/* Time Machine CoW Banner */}
              <div className="rounded-2xl border border-accent/40 bg-accent/5 p-6 shadow-xs flex flex-col justify-between gap-4 sm:flex-row sm:items-center">
                <div>
                  <h3 className="text-lg font-bold text-fg flex items-center gap-2">
                    <Clock className="text-accent" size={20} />
                    {content.pmbs.timeMachineTitle}
                  </h3>
                  <p className="mt-1 text-xs text-fg-muted leading-relaxed max-w-2xl">{content.pmbs.timeMachineDesc}</p>
                </div>
                <button
                  type="button"
                  onClick={() => setActiveTab('downloads')}
                  className="rounded-xl bg-accent px-4 py-2.5 text-xs font-bold text-accent-fg shadow-sm hover:bg-accent-hover shrink-0"
                >
                  Voir Commandes PMBS CLI
                </button>
              </div>
            </motion.div>
          )}

          {/* TAB 3: MESHFS */}
          {activeTab === 'meshfs' && (
            <motion.div
              key="meshfs"
              initial={{ opacity: 0, y: 10 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -10 }}
              transition={{ duration: 0.2 }}
              className="space-y-10"
            >
              <div className="rounded-3xl border border-line bg-surface p-6 sm:p-8 shadow-xs">
                <span className="rounded-full bg-accent/15 px-3 py-1 text-xs font-bold text-accent">
                  {content.meshfs.tag}
                </span>
                <h2 className="mt-3 text-2xl font-black text-fg sm:text-3xl">{content.meshfs.title}</h2>
                <p className="mt-2 text-sm leading-relaxed text-fg-muted sm:text-base">{content.meshfs.lead}</p>
              </div>

              {/* 3 Perspectives : Humain (Non-Geek) / Geek (Kernel) / Agent IA (Machine) */}
              <TechnologyPerspectiveCards
                perspectives={content.meshfs?.perspectives}
                currentPerspective={currentPerspective}
              />

              {/* SVG Moteur Distribué Dédupliqué & Arbre de Merkle */}
              <MeshFsSvgDiagram />

              {/* Merkle Tree & Multi Engines Grid */}
              <div className="grid gap-6 md:grid-cols-2">
                <div className="rounded-2xl border border-line bg-surface p-6 shadow-xs space-y-3">
                  <div className="flex items-center gap-2">
                    <FolderTree className="text-accent" size={20} />
                    <h3 className="text-lg font-bold text-fg">{content.meshfs.merkleTitle}</h3>
                  </div>
                  <p className="text-xs text-fg-muted leading-relaxed">{content.meshfs.merkleDesc}</p>

                  {/* Merkle Tree Visual Diagram */}
                  <div className="rounded-xl border border-line bg-canvas p-4 font-mono text-[11px] space-y-2">
                    <div className="flex items-center gap-2 text-accent font-bold">
                      <span className="h-2 w-2 rounded-full bg-accent" />
                      Root Merkle Hash: 958e0b74a1...
                    </div>
                    <div className="ml-4 border-l border-line pl-3 space-y-1.5 text-fg-muted">
                      <div>├── /etc/shaper [Hash: 443dedd...]</div>
                      <div>│   └── status.json [Chunk 4MB: d4e6a7c...]</div>
                      <div>└── /var/lib/mariadb [Hash: 1f2a3b4...]</div>
                      <div>    ├── ibdata1 [Chunks: CAS#1, CAS#2, CAS#3]</div>
                      <div>    └── binlog.000042 [Chunk 4MB: a9b0c1d...]</div>
                    </div>
                  </div>
                </div>

                <div className="rounded-2xl border border-line bg-surface p-6 shadow-xs space-y-3">
                  <div className="flex items-center gap-2">
                    <Boxes className="text-emerald-500" size={20} />
                    <h3 className="text-lg font-bold text-fg">{content.meshfs.enginesTitle}</h3>
                  </div>
                  <p className="text-xs text-fg-muted leading-relaxed">{content.meshfs.enginesDesc}</p>

                  <div className="grid grid-cols-2 gap-2 pt-2">
                    {[
                      { name: 'ZFS Pool (zpool)', note: 'ARC Cache & Scrub natif' },
                      { name: 'Btrfs CoW', note: 'Subvolumes & Send/Receive' },
                      { name: 'LVM Thin-Pool', note: 'Provisionnement élastique' },
                      { name: 'ext4 Standard', note: 'Compatibilité universelle' },
                    ].map((eng) => (
                      <div key={eng.name} className="rounded-xl border border-line bg-surface-2 p-3">
                        <p className="font-bold text-xs text-fg">{eng.name}</p>
                        <p className="text-[11px] text-fg-muted mt-0.5">{eng.note}</p>
                      </div>
                    ))}
                  </div>
                </div>
              </div>

              {/* CEPHFS VS MESHFS UNCOMPROMISING BENCHMARK */}
              <section className="space-y-4">
                <div className="flex items-center justify-between">
                  <h3 className="text-xl font-black text-fg flex items-center gap-2">
                    <Flame className="text-danger" size={22} />
                    {content.meshfs.cephComparisonTitle}
                  </h3>
                  <span className="rounded-full bg-danger/10 px-3 py-1 text-xs font-bold text-danger">
                    Zéro Dépendance OSD
                  </span>
                </div>

                <div className="overflow-x-auto rounded-2xl border border-line bg-surface shadow-xs">
                  <table className="w-full text-left text-xs">
                    <thead className="border-b border-line bg-surface-2/60 text-fg-muted uppercase tracking-wider text-[10px]">
                      <tr>
                        {content.meshfs.cephComparisonHeaders.map((head) => (
                          <th key={head} className="px-4 py-3 font-bold">
                            {head}
                          </th>
                        ))}
                      </tr>
                    </thead>
                    <tbody className="divide-y divide-line/60">
                      {content.meshfs.cephComparisonRows.map((row, idx) => (
                        <tr key={idx} className="hover:bg-surface-2/40 transition-colors">
                          <td className="px-4 py-3.5 font-bold text-fg">{row[0]}</td>
                          <td className="px-4 py-3.5 text-danger font-medium">{row[1]}</td>
                          <td className="px-4 py-3.5 text-emerald-600 dark:text-emerald-400 font-bold">{row[2]}</td>
                          <td className="px-4 py-3.5 font-semibold text-accent">{row[3]}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </section>
            </motion.div>
          )}

          {/* TAB 4: PROXMOX VE PLUGIN */}
          {activeTab === 'proxmox' && (
            <motion.div
              key="proxmox"
              initial={{ opacity: 0, y: 10 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -10 }}
              transition={{ duration: 0.2 }}
              className="space-y-10"
            >
              <div className="rounded-3xl border border-line bg-surface p-6 sm:p-8 shadow-xs">
                <span className="rounded-full bg-sky-500/15 px-3 py-1 text-xs font-bold text-sky-500">
                  {content.proxmox.tag}
                </span>
                <h2 className="mt-3 text-2xl font-black text-fg sm:text-3xl">{content.proxmox.title}</h2>
                <p className="mt-2 text-sm leading-relaxed text-fg-muted sm:text-base">{content.proxmox.lead}</p>
              </div>

              {/* 3 Perspectives : Humain (Non-Geek) / Geek (Kernel) / Agent IA (Machine) */}
              <TechnologyPerspectiveCards
                perspectives={content.proxmox?.perspectives}
                currentPerspective={currentPerspective}
              />

              {/* 3 Core Proxmox Features */}
              <div className="grid gap-4 md:grid-cols-3">
                {content.proxmox.features.map((feat) => (
                  <div key={feat.title} className="rounded-2xl border border-line bg-surface p-5 shadow-xs space-y-2">
                    <div className="flex h-9 w-9 items-center justify-center rounded-xl bg-sky-500/10 text-sky-500">
                      <Server size={20} />
                    </div>
                    <h4 className="text-base font-bold text-fg">{feat.title}</h4>
                    <p className="text-xs text-fg-muted leading-relaxed">{feat.desc}</p>
                  </div>
                ))}
              </div>

              {/* 1-Click Install Command Box */}
              <section className="rounded-2xl border border-line bg-surface p-6 shadow-xs space-y-4">
                <div className="flex flex-col justify-between gap-2 sm:flex-row sm:items-center">
                  <div>
                    <h3 className="text-lg font-bold text-fg flex items-center gap-2">
                      <Terminal className="text-accent" size={20} />
                      {content.proxmox.quickInstallTitle}
                    </h3>
                    <p className="text-xs text-fg-muted mt-1">{content.proxmox.quickInstallDesc}</p>
                  </div>
                </div>

                <div className="relative rounded-xl border border-line bg-canvas p-4 font-mono text-xs text-fg">
                  <div className="flex items-center justify-between">
                    <span className="text-emerald-500 font-bold"># Proxmox VE 8.x / 9.x Host Terminal</span>
                    <button
                      type="button"
                      onClick={() =>
                        copyToClipboard(
                          'curl -sSL https://get.podmesh.io/pve-install.sh | bash',
                          'pve-cmd'
                        )
                      }
                      className="flex items-center gap-1.5 rounded-lg border border-line bg-surface px-2.5 py-1 text-[11px] font-semibold text-fg hover:bg-surface-2"
                    >
                      {copiedText === 'pve-cmd' ? <Check size={13} className="text-emerald-500" /> : <Copy size={13} />}
                      <span>{copiedText === 'pve-cmd' ? 'Copié !' : 'Copier'}</span>
                    </button>
                  </div>
                  <pre className="mt-3 text-accent font-bold select-all">
                    curl -sSL https://get.podmesh.io/pve-install.sh | bash
                  </pre>
                </div>

                {/* Storage.cfg Snippet */}
                <div className="rounded-xl border border-line bg-surface-2/40 p-4 space-y-2">
                  <div className="flex items-center justify-between text-xs font-bold text-fg">
                    <span>Configuration automatique dans /etc/pve/storage.cfg</span>
                    <span className="font-mono text-[11px] text-fg-muted">plugin: PVE::Storage::Custom::PodMeshPlugin</span>
                  </div>
                  <pre className="rounded-lg bg-canvas p-3 font-mono text-[11px] text-fg-muted overflow-x-auto leading-relaxed">
{`meshfs: podmesh-shared
        path /meshfs/universes
        content images,rootdir,vzdump
        nodes pve-node-01,pve-node-02
        shared 1
        prune-backups keep-daily=7,keep-weekly=4,keep-monthly=12`}
                  </pre>
                </div>
              </section>
            </motion.div>
          )}

          {/* TAB 5: DOWNLOADS & CLI */}
          {activeTab === 'downloads' && (
            <motion.div
              key="downloads"
              initial={{ opacity: 0, y: 10 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -10 }}
              transition={{ duration: 0.2 }}
              className="space-y-10"
            >
              <div className="rounded-3xl border border-line bg-surface p-6 sm:p-8 shadow-xs">
                <span className="rounded-full bg-accent/15 px-3 py-1 text-xs font-bold text-accent">
                  {content.downloads.tag}
                </span>
                <h2 className="mt-3 text-2xl font-black text-fg sm:text-3xl">{content.downloads.title}</h2>
                <p className="mt-2 text-sm leading-relaxed text-fg-muted sm:text-base">{content.downloads.lead}</p>
              </div>

              {/* APT Quickstart One-Liner */}
              <div className="rounded-2xl border border-line bg-surface p-5 shadow-xs space-y-3">
                <h3 className="text-sm font-bold text-fg flex items-center gap-2">
                  <Zap className="text-amber-500" size={16} /> Configuration Rapide du Dépôt APT (Debian 12 / Ubuntu 24.04)
                </h3>
                <div className="relative rounded-xl border border-line bg-canvas p-3 font-mono text-xs text-fg">
                  <div className="flex items-center justify-between">
                    <code className="text-fg-muted">
                      curl -fsSL https://deb.podmesh.io/gpg.key | gpg --dearmor -o /etc/apt/keyrings/podmesh.gpg && \<br />
                      echo &quot;deb [signed-by=/etc/apt/keyrings/podmesh.gpg] https://deb.podmesh.io stable main&quot; &gt; /etc/apt/sources.list.d/podmesh.list && apt update
                    </code>
                    <button
                      type="button"
                      onClick={() =>
                        copyToClipboard(
                          'curl -fsSL https://deb.podmesh.io/gpg.key | gpg --dearmor -o /etc/apt/keyrings/podmesh.gpg && echo "deb [signed-by=/etc/apt/keyrings/podmesh.gpg] https://deb.podmesh.io stable main" > /etc/apt/sources.list.d/podmesh.list && apt update',
                          'apt-setup'
                        )
                      }
                      className="ml-2 flex shrink-0 items-center gap-1 rounded-md border border-line bg-surface px-2 py-1 text-[11px] font-semibold text-fg hover:bg-surface-2"
                    >
                      {copiedText === 'apt-setup' ? <Check size={12} className="text-emerald-500" /> : <Copy size={12} />}
                      <span>{copiedText === 'apt-setup' ? 'Copié' : 'Copier'}</span>
                    </button>
                  </div>
                </div>
              </div>

              {/* Debian Packages List */}
              <section className="space-y-4">
                <h3 className="text-xl font-black text-fg flex items-center gap-2">
                  <Download className="text-accent" size={22} />
                  {content.downloads.packagesTitle}
                </h3>
                <div className="grid gap-4 md:grid-cols-2">
                  {content.downloads.packages.map((pkg) => (
                    <div key={pkg.name} className="rounded-2xl border border-line bg-surface p-5 shadow-xs space-y-3">
                      <div className="flex items-start justify-between">
                        <div>
                          <div className="flex items-center gap-2">
                            <span className="font-mono font-bold text-sm text-fg">{pkg.name}</span>
                            <span className="rounded bg-accent/10 px-1.5 py-0.5 text-[10px] font-bold text-accent">
                              v{pkg.version}
                            </span>
                            <span className="rounded bg-surface-2 px-1.5 py-0.5 text-[10px] font-mono text-fg-muted">
                              {pkg.arch}
                            </span>
                          </div>
                          <p className="mt-1 text-xs text-fg-muted">{pkg.desc}</p>
                        </div>
                        <span className="text-[11px] font-mono text-fg-muted shrink-0">{pkg.size}</span>
                      </div>

                      <div className="rounded-xl border border-line bg-canvas p-2.5 font-mono text-[11px] flex items-center justify-between">
                        <span className="text-fg-muted truncate">apt install -y {pkg.name}</span>
                        <button
                          type="button"
                          onClick={() => copyToClipboard(`apt install -y ${pkg.name}`, pkg.name)}
                          className="flex items-center gap-1 text-[11px] text-accent font-semibold hover:underline shrink-0 ml-2"
                        >
                          {copiedText === pkg.name ? <Check size={12} className="text-emerald-500" /> : <Copy size={12} />}
                          <span>{copiedText === pkg.name ? 'Copié' : 'Copier'}</span>
                        </button>
                      </div>
                    </div>
                  ))}
                </div>
              </section>

              {/* CLI Cheatsheet Interactive Terminal */}
              <section className="rounded-2xl border border-line bg-surface p-6 shadow-xs space-y-4">
                <div className="flex flex-col justify-between gap-3 sm:flex-row sm:items-center">
                  <div>
                    <h3 className="text-xl font-black text-fg flex items-center gap-2">
                      <Terminal className="text-accent" size={22} />
                      {content.downloads.cliTitle}
                    </h3>
                    <p className="text-xs text-fg-muted mt-1">
                      Commandes administratives validées sous SHAPER OS v1.15.
                    </p>
                  </div>

                  <div className="flex items-center gap-1.5 rounded-xl border border-line bg-surface-2 p-1">
                    {['podmesh', 'pmbs', 'meshfs'].map((cat) => (
                      <button
                        key={cat}
                        type="button"
                        onClick={() => setCliCategory(cat)}
                        className={`rounded-lg px-3 py-1 text-xs font-mono font-bold transition-all ${
                          cliCategory === cat ? 'bg-accent text-accent-fg shadow-xs' : 'text-fg-muted hover:text-fg'
                        }`}
                      >
                        {cat}
                      </button>
                    ))}
                  </div>
                </div>

                <div className="space-y-3">
                  {(cliSnippets[cliCategory] || []).map((snippet, idx) => (
                    <div
                      key={snippet.cmd}
                      className="rounded-xl border border-line bg-canvas p-4 font-mono text-xs transition-all hover:border-line-strong"
                    >
                      <div className="flex items-center justify-between text-fg-muted">
                        <span className="font-sans font-semibold text-fg text-xs">{snippet.title}</span>
                        <button
                          type="button"
                          onClick={() => copyToClipboard(snippet.cmd, `cli-${cliCategory}-${idx}`)}
                          className="flex items-center gap-1 rounded-md border border-line bg-surface px-2 py-0.5 text-[10px] text-fg hover:bg-surface-2"
                        >
                          {copiedText === `cli-${cliCategory}-${idx}` ? (
                            <Check size={12} className="text-emerald-500" />
                          ) : (
                            <Copy size={12} />
                          )}
                          <span>{copiedText === `cli-${cliCategory}-${idx}` ? 'Copié' : 'Copier'}</span>
                        </button>
                      </div>
                      <p className="font-sans text-[11px] text-fg-muted mt-1 mb-2.5">{snippet.desc}</p>
                      <div className="rounded-lg bg-surface-2/60 p-2 text-accent font-bold select-all overflow-x-auto">
                        $ {snippet.cmd}
                      </div>
                    </div>
                  ))}
                </div>
              </section>
            </motion.div>
          )}
        </AnimatePresence>

          {/* Bottom Chapter Navigation */}
          <div className="mt-12 flex flex-col sm:flex-row items-center justify-between gap-4 border-t border-line pt-6">
            {prevTab ? (
              <button
                type="button"
                onClick={() => {
                  setActiveTab(prevTab)
                  window.scrollTo({ top: 380, behavior: 'smooth' })
                }}
                className="flex items-center gap-2 rounded-xl border border-line bg-surface px-4 py-2.5 text-xs font-bold text-fg hover:bg-surface-2 transition-colors shadow-xs"
              >
                <ChevronLeft size={16} />
                <span>{content.navTabs[prevTab]}</span>
              </button>
            ) : <div />}

            {nextTab ? (
              <button
                type="button"
                onClick={() => {
                  setActiveTab(nextTab)
                  window.scrollTo({ top: 380, behavior: 'smooth' })
                }}
                className="flex items-center gap-2 rounded-xl bg-accent px-4 py-2.5 text-xs font-bold text-accent-fg hover:bg-accent-hover transition-colors shadow-sm ml-auto"
              >
                <span>{content.navTabs[nextTab]}</span>
                <ChevronRight size={16} />
              </button>
            ) : <div />}
          </div>
        </main>
      </div>

      {/* Footer */}
      <footer className="mt-16 border-t border-line bg-surface/80 py-10">
        <div className="mx-auto flex max-w-7xl flex-col items-center justify-between gap-4 px-4 sm:flex-row sm:px-6 lg:px-8">
          <div className="flex items-center gap-3">
            <span className="site-display text-lg font-black tracking-tight text-fg">PodMesh</span>
            <div className="h-4 w-[1px] bg-line opacity-80" aria-hidden="true" />
            <span className="text-xs text-fg-muted font-medium">Documentation Technique Publique &amp; Spécifications Souveraines</span>
          </div>
          <div className="flex items-center gap-4 text-xs text-fg-muted">
            <span>SHAPER OS v1.15</span>
            <span>•</span>
            <span>MeshFS &amp; PMBS Multi-Sites</span>
            <span>•</span>
            <span className="text-accent font-semibold">Open Technical Portal</span>
          </div>
        </div>
      </footer>
    </div>
  )
}
