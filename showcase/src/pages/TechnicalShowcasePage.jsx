import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { Link, useLocation, useNavigate } from 'react-router-dom'
import { ArrowRight, ArrowLeft, Bot, BookOpen, ChevronDown, ExternalLink, Menu } from 'lucide-react'
import { useI18n } from '../i18n/I18nProvider'
import LanguageSwitcher from '../components/LanguageSwitcher'
import ThemeToggle from '../components/ui/ThemeToggle'
import ShowcaseSidebar, { SIDEBAR_ITEMS } from '../components/showcase/ShowcaseSidebar'
import GlossaryText from '../components/GlossaryText'
import ComponentDiagram from '../components/showcase/ComponentDiagram'
import { guideContent } from '../data/publicGuideContent'
import ValidationPage from './ValidationPage'
import { validationCopy } from '../data/validationCopy'

const REPOSITORY = 'https://github.com/xavdp-pro/podmesh'
const DOC_BASE = `${REPOSITORY}/tree/cursor/assembly-2026-10-07/`
// Keep bookmarks from the previous page useful after the editorial rewrite.
const LEGACY_ANCHORS = {
  'strata-law': 'podmesh-details', 'strata-diagram': 'podmesh-details', 'strata-isolation': 'podmesh-details',
  'meshfs-merkle': 'meshfs-details', 'meshfs-engines': 'meshfs-details', 'meshfs-ceph-comparison': 'meshfs-usage',
  'pmbs-cas': 'pmbs-details', 'pmbs-op10': 'pmbs-details', 'pmbs-remotesync': 'pmbs-agent',
  'pve-driver': 'proxmox-details', 'pve-config': 'proxmox-agent', 'pve-workloads': 'proxmox-usage',
  'cli-deb': 'downloads-details', 'cli-apt': 'downloads-details', 'cli-oci': 'downloads-usage', 'cli-terminal': 'downloads-agent',
}

export default function TechnicalShowcasePage() {
  const { locale } = useI18n()
  const copy = guideContent[locale] || guideContent.fr
  const { hash, pathname } = useLocation()
  const navigate = useNavigate()
  const targetId = LEGACY_ANCHORS[hash.slice(1)] || hash.slice(1)
  const isValidation = pathname === '/validation' || targetId.startsWith('validation')
  const activeId = isValidation ? 'validation' : SIDEBAR_ITEMS.find((item) => item.key === targetId || item.anchors.some((anchor) => anchor.id === targetId))?.key || 'podmesh'
  const index = copy.sections.findIndex((section) => section.id === activeId)
  const validationText = validationCopy[locale] || validationCopy.fr
  const section = isValidation ? { name: validationText.nav } : copy.sections[index]
  const [mobileOpen, setMobileOpen] = useState(false)
  const [isDesktop, setIsDesktop] = useState(() => window.matchMedia('(min-width: 1024px)').matches)
  const [desktopExpanded, setDesktopExpanded] = useState(() => {
    try { return localStorage.getItem('showcase:desktop-expanded') !== 'false' } catch { return true }
  })
  function setDesktopMenu(expanded) {
    setDesktopExpanded(expanded)
    try { localStorage.setItem('showcase:desktop-expanded', String(expanded)) } catch {}
  }
  useEffect(() => {
    const media = window.matchMedia('(min-width: 1024px)')
    const changed = () => { setIsDesktop(media.matches); setMobileOpen(false) }
    media.addEventListener('change', changed)
    return () => media.removeEventListener('change', changed)
  }, [])
  const menuRef = useRef(null)
  const text = (value) => <GlossaryText lang={locale}>{value}</GlossaryText>

  function followTarget(id) {
    const target = document.getElementById(id)
    if (!target) return
    // A link to a reading level opens it before scrolling to its heading.
    if (target.tagName === 'DETAILS') target.open = true
    target.focus({ preventScroll: true })
    target.scrollIntoView({ block: 'start', behavior: 'instant' })
  }

  function selectSection(id, anchorId = id) {
    setMobileOpen(false)
    const destination = id === 'validation' ? '/validation' : '/'
    if (pathname === destination && targetId === anchorId) followTarget(anchorId)
    else navigate({ pathname: destination, hash: `#${anchorId}` })
  }

  useLayoutEffect(() => {
    if (targetId) followTarget(targetId)
    else if (isValidation) window.scrollTo({ top: 0, behavior: 'instant' })
  }, [targetId, activeId, locale, isValidation])

  useEffect(() => {
    document.title = `${section.name} · PodMesh`
    document.querySelector('meta[name="description"]')?.setAttribute('content', isValidation ? validationText.intro : copy.meta)
  }, [section.name, copy.meta, isValidation, validationText.intro])

  useEffect(() => {
    if (!mobileOpen) return
    const menu = document.querySelector('[data-mobile-guide]')
    menu?.querySelector('button, a')?.focus()
    const originalOverflow = document.body.style.overflow
    document.body.style.overflow = 'hidden'
    const trapFocus = (event) => {
      if (event.key !== 'Tab' || !menu) return
      const controls = [...menu.querySelectorAll('button, a[href]')]
      const first = controls[0], last = controls.at(-1)
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus() }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus() }
    }
    document.addEventListener('keydown', trapFocus)
    return () => {
      document.body.style.overflow = originalOverflow
      document.removeEventListener('keydown', trapFocus)
      if (menu?.contains(document.activeElement)) menuRef.current?.focus()
    }
  }, [mobileOpen])

  return (
    <div className="min-h-screen bg-canvas text-fg">
      <header className="sticky top-0 z-40 border-b border-line bg-surface/95 backdrop-blur-md">
        <div className="mx-auto flex min-h-16 max-w-7xl flex-wrap items-center justify-between gap-2 px-4 py-2 sm:px-6 lg:px-8">
          <div className="flex items-center gap-3">
            <button ref={menuRef} type="button" onClick={() => isDesktop ? setDesktopMenu(!desktopExpanded) : setMobileOpen(true)} aria-label={isDesktop && desktopExpanded ? copy.close : copy.menu} aria-expanded={isDesktop ? desktopExpanded : mobileOpen} aria-controls={isDesktop ? "guide-navigation-desktop" : "guide-navigation"} className="rounded-lg border border-line p-2"><Menu size={18} /></button>

            <Link to="/#podmesh" className="site-display text-2xl font-bold">PodMesh</Link>
            <a href="https://xavdp.pro" target="_blank" rel="noopener noreferrer" className="hidden border-l border-line pl-3 text-sm text-fg-muted sm:block">xavdp.pro</a>
          </div>
          <div className="flex items-center gap-2"><LanguageSwitcher /><ThemeToggle /></div>
        </div>
      </header>

      <div className="mx-auto flex max-w-7xl items-start gap-6 px-4 py-6 sm:px-6 lg:gap-10 lg:px-8">
        <ShowcaseSidebar activeTab={activeId} activeAnchor={targetId} onSelectTab={selectSection} desktopExpanded={desktopExpanded} onCollapseDesktop={() => { setDesktopMenu(false); menuRef.current?.focus() }} mobileOpen={mobileOpen} onCloseMobile={() => { setMobileOpen(false); menuRef.current?.focus() }} locale={locale} />
        <main className="min-w-0 flex-1 pb-12">
          {!isValidation && <section className="mb-10 border-b border-line pb-8">
            <p className="mb-4 text-sm font-semibold text-accent">PodMesh · MeshFS · PMBS</p>
            <h1 className="site-display max-w-3xl text-4xl font-semibold leading-tight sm:text-5xl">{text(copy.title)}</h1>
            <p className="mt-5 max-w-3xl text-lg leading-8 text-fg-muted">{text(copy.intro)}</p>
            <p className="mt-4 max-w-3xl text-base font-semibold leading-7">{text(copy.overview)}</p>
            <p className="mt-4 max-w-3xl text-base leading-7">{text(copy.tandem)}</p>
            <p className="mt-4 text-sm leading-6 text-fg-muted">{copy.direction}{' '}<a href={`${DOC_BASE}docs/IDEAL-SCENE.md`} target="_blank" rel="noreferrer" className="text-accent underline underline-offset-4">{copy.directionLink}</a></p>
            <p className="mt-5 border-l-2 border-warm pl-4 text-sm leading-6 text-fg-muted">{text(copy.vocabulary)}</p>
            <p className="mt-3 text-xs text-fg-faint">{locale === 'fr' ? 'Un mot souligné ? Survolez-le ou touchez-le pour l’expliquer.' : locale === 'es' ? '¿Una palabra subrayada? Pasa el cursor o tócala para ver su explicación.' : 'An underlined word? Hover or tap it for an explanation.'}</p>
            <Link to="/validation" className="mt-5 inline-flex items-center gap-2 text-sm font-semibold text-accent underline underline-offset-4">{validationText.nav}<ArrowRight size={16} aria-hidden="true" /></Link>
          </section>}

          {isValidation ? <ValidationPage locale={locale} /> : <article key={`${activeId}-${locale}`} id={activeId} tabIndex={-1} className="scroll-mt-24 space-y-7">
            <div>
              <p className="mb-3 text-sm font-semibold text-accent">{section.name} · {section.short}</p>
              <h2 className="site-display text-3xl font-semibold leading-tight sm:text-4xl">{section.title}</h2>
              <p className="mt-4 text-lg leading-8 text-fg-muted">{text(section.lead)}</p>
            </div>

            <ComponentDiagram section={activeId} locale={locale} />

            <section id={`${activeId}-usage`} tabIndex={-1} className="scroll-mt-24 rounded-2xl border border-line bg-surface px-5 py-6 sm:p-7">
              <h3 className="mb-5 text-lg font-semibold">{copy.usage}</h3>
              <div className="space-y-6">
                {section.points.map(([title, body], pointIndex) => (
                  <div key={title} className="flex gap-4">
                    <span aria-hidden="true" className="pt-1 text-sm font-semibold text-warm">{String(pointIndex + 1).padStart(2, '0')}</span>
                    <div><h4 className="text-base font-semibold">{title}</h4><p className="mt-1 text-base leading-7 text-fg-muted">{text(body)}</p></div>
                  </div>
                ))}
              </div>
            </section>

            <aside className="rounded-2xl border border-line bg-warm-soft p-5 sm:p-7">
              <h3 className="mb-2 text-base font-semibold text-warm">{copy.example}</h3>
              <p className="text-base leading-7">{text(section.example)}</p>
            </aside>

            <details id={`${activeId}-agent`} tabIndex={-1} className="group scroll-mt-24 rounded-2xl border border-line bg-surface p-5 sm:p-7">
              <summary className="flex items-center gap-3 text-base font-semibold"><Bot size={21} className="shrink-0 text-signal" />{copy.agent}<ChevronDown size={18} className="ml-auto shrink-0 group-open:rotate-180" /></summary>
              <ol className="mt-5 list-decimal space-y-3 pl-6 text-base leading-7 text-fg-muted">{section.workflow.map((step) => <li key={step}>{text(step)}</li>)}</ol>
            </details>

            <details id={`${activeId}-details`} tabIndex={-1} className="group scroll-mt-24 rounded-2xl border border-line bg-surface p-5 sm:p-7">
              <summary className="flex items-center gap-3 text-base font-semibold"><BookOpen size={20} className="shrink-0 text-accent" />{copy.details}<ChevronDown size={18} className="ml-auto shrink-0 group-open:rotate-180" /></summary>
              <p className="mt-5 text-base leading-7 text-fg-muted">{text(section.technical)}</p>
              <a href={`${DOC_BASE}${section.doc}`} target="_blank" rel="noreferrer" className="mt-4 inline-flex items-center gap-2 text-sm font-semibold text-accent underline underline-offset-4">{section.docLabel || copy.docs}<ExternalLink size={14} /></a>
            </details>

            <nav aria-label={copy.summary} className="flex flex-wrap justify-between gap-4 border-t border-line pt-6">
              {index > 0 ? <Link to={`#${copy.sections[index - 1].id}`} className="inline-flex items-center gap-2 text-sm font-semibold text-accent"><ArrowLeft size={16} />{copy.previous} {copy.sections[index - 1].name}</Link> : <span />}
              {index < copy.sections.length - 1 && <Link to={`#${copy.sections[index + 1].id}`} className="inline-flex items-center gap-2 text-sm font-semibold text-accent">{copy.next} {copy.sections[index + 1].name}<ArrowRight size={16} /></Link>}
            </nav>
          </article>}
        </main>
      </div>
      <footer className="border-t border-line bg-surface px-4 py-6 text-sm text-fg-muted">
        <div className="mx-auto flex max-w-7xl flex-wrap items-center justify-between gap-3"><p>{copy.footer}</p><a href={REPOSITORY} target="_blank" rel="noreferrer" className="text-accent underline underline-offset-4">{copy.source} · GitHub</a></div>
      </footer>
    </div>
  )
}
