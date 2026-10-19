import React from 'react'
import {
  User,
  Cpu,
  Bot,
  Layers,
  Sparkles,
  ExternalLink,
  Code2,
  CheckCircle2,
  GitBranch,
} from 'lucide-react'

// GitHub SVG Logo
export function GitHubIcon({ size = 18, className = '' }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="currentColor"
      className={className}
      aria-hidden="true"
    >
      <path
        fillRule="evenodd"
        clipRule="evenodd"
        d="M12 2C6.477 2 2 6.484 2 12.017c0 4.425 2.865 8.18 6.839 9.504.5.092.682-.217.682-.483 0-.237-.008-.868-.013-1.703-2.782.605-3.369-1.343-3.369-1.343-.454-1.158-1.11-1.466-1.11-1.466-.908-.62.069-.608.069-.608 1.003.07 1.53 1.032 1.53 1.032.892 1.53 2.341 1.088 2.91.832.092-.647.35-1.088.636-1.338-2.22-.253-4.555-1.113-4.555-4.951 0-1.093.39-1.988 1.029-2.688-.103-.253-.446-1.272.098-2.65 0 0 .84-.27 2.75 1.026A9.564 9.564 0 0112 6.844c.85.004 1.705.115 2.504.337 1.909-1.296 2.747-1.027 2.747-1.027.546 1.379.202 2.398.1 2.651.64.7 1.028 1.595 1.028 2.688 0 3.848-2.339 4.695-4.566 4.943.359.309.678.92.678 1.855 0 1.338-.012 2.419-.012 2.747 0 .268.18.58.688.482A10.019 10.019 0 0022 12.017C22 6.484 17.522 2 12 2z"
      />
    </svg>
  )
}

/**
 * Bannière doctrinale et sélecteur de perspective Humain ⇄ Agent IA.
 */
export default function PerspectiveTandemSection({
  tandemData,
  githubData,
  currentPerspective = 'all',
  onSelectPerspective,
  locale = 'fr',
}) {
  if (!tandemData) return null

  const PERSPECTIVES = [
    {
      id: 'all',
      label: { fr: 'Tous les Niveaux', en: 'All Perspectives', es: 'Todos los Niveles' },
      icon: Layers,
      color: 'accent',
    },
    {
      id: 'human',
      label: { fr: 'Humain & Décideur (Pas Geek)', en: 'Human / Decision Maker', es: 'Humano / Decisor' },
      icon: User,
      color: 'emerald-500',
    },
    {
      id: 'geek',
      label: { fr: 'Ingénieur Système (Geek)', en: 'Systems Engineer (Geek)', es: 'Ingeniero / Geek' },
      icon: Cpu,
      color: 'sky-500',
    },
    {
      id: 'agent',
      label: { fr: 'Agent IA & Machine (Tandem)', en: 'AI Agent & Machine', es: 'Agente IA / Máquina' },
      icon: Bot,
      color: 'amber-500',
    },
  ]

  return (
    <section className="rounded-3xl border border-line bg-surface p-6 sm:p-8 shadow-xs space-y-6">
      {/* Header Doctrine Tandem */}
      <div className="flex flex-col justify-between gap-4 lg:flex-row lg:items-center border-b border-line pb-6">
        <div className="max-w-2xl">
          <div className="inline-flex items-center gap-2 rounded-full border border-accent/30 bg-accent/10 px-3 py-1 text-xs font-bold text-accent shadow-xs mb-3">
            <Sparkles size={13} />
            <span>{tandemData.badge}</span>
          </div>
          <h2 className="site-display text-2xl font-black text-fg sm:text-3xl tracking-tight">
            {tandemData.title}
          </h2>
          <p className="mt-2 text-xs leading-relaxed text-fg-muted sm:text-sm font-normal">
            {tandemData.lead}
          </p>
        </div>

        {/* Liens GitHub Officiels */}
        {githubData && (
          <div className="flex flex-col sm:flex-row lg:flex-col gap-2.5 shrink-0">
            <a
              href={githubData.podmeshUrl}
              target="_blank"
              rel="noopener noreferrer"
              className="flex items-center justify-between gap-3 rounded-xl border border-line bg-surface-2/70 px-4 py-2.5 text-xs font-bold text-fg hover:bg-surface-2 hover:border-line-strong transition-all shadow-xs group"
            >
              <div className="flex items-center gap-2.5">
                <GitHubIcon size={17} className="text-fg transition-transform group-hover:scale-110" />
                <span>{githubData.podmeshLabel}</span>
              </div>
              <ExternalLink size={13} className="text-fg-faint opacity-60 group-hover:opacity-100" />
            </a>

            <a
              href={githubData.shaperUrl}
              target="_blank"
              rel="noopener noreferrer"
              className="flex items-center justify-between gap-3 rounded-xl border border-line bg-surface-2/70 px-4 py-2.5 text-xs font-bold text-fg hover:bg-surface-2 hover:border-line-strong transition-all shadow-xs group"
            >
              <div className="flex items-center gap-2.5">
                <GitBranch size={17} className="text-accent transition-transform group-hover:scale-110" />
                <span>{githubData.shaperLabel}</span>
              </div>
              <ExternalLink size={13} className="text-fg-faint opacity-60 group-hover:opacity-100" />
            </a>
          </div>
        )}
      </div>

      {/* 3 Niveaux de Lecture Explicatifs */}
      <div className="grid gap-3 sm:grid-cols-3">
        {tandemData.levels?.map((lvl) => {
          const isSelected = currentPerspective === lvl.id
          return (
            <button
              key={lvl.id}
              type="button"
              onClick={() => onSelectPerspective(lvl.id)}
              className={`rounded-2xl border p-4 text-left transition-all ${
                isSelected
                  ? 'border-accent bg-accent/10 ring-1 ring-accent shadow-xs'
                  : 'border-line bg-canvas hover:border-line-strong hover:bg-surface-2/50'
              }`}
            >
              <div className="flex items-center justify-between">
                <span className="rounded-full bg-surface px-2.5 py-0.5 text-[10px] font-bold uppercase tracking-wider text-accent border border-line">
                  {lvl.badge}
                </span>
                {isSelected && <CheckCircle2 size={16} className="text-accent" />}
              </div>
              <h4 className="mt-2.5 text-sm font-bold text-fg">{lvl.name}</h4>
              <p className="mt-0.5 text-[11px] font-semibold text-accent">{lvl.role}</p>
              <p className="mt-2 text-xs text-fg-muted leading-relaxed">{lvl.summary}</p>
            </button>
          )
        })}
      </div>

      {/* Barre de Filtrage par Niveau */}
      <div className="flex flex-wrap items-center justify-between gap-3 pt-2">
        <span className="text-xs font-bold text-fg-faint uppercase tracking-wider">
          {locale === 'en' ? 'Filter reading level:' : locale === 'es' ? 'Filtrar nivel de lectura:' : 'Niveau de lecture :'}
        </span>
        <div className="inline-flex flex-wrap gap-1.5 rounded-xl border border-line bg-surface-2/60 p-1">
          {PERSPECTIVES.map((p) => {
            const Icon = p.icon
            const active = currentPerspective === p.id
            const label = p.label[locale] || p.label.fr
            return (
              <button
                key={p.id}
                type="button"
                onClick={() => onSelectPerspective(p.id)}
                className={`flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-bold transition-all ${
                  active
                    ? 'bg-accent text-accent-fg shadow-xs'
                    : 'text-fg-muted hover:text-fg hover:bg-surface'
                }`}
              >
                <Icon size={14} />
                <span>{label}</span>
              </button>
            )
          })}
        </div>
      </div>
    </section>
  )
}

/**
 * Bloc de perspective modulaire pour une technologie spécifique.
 */
export function TechnologyPerspectiveCards({ perspectives, currentPerspective = 'all' }) {
  if (!perspectives) return null

  const showHuman = currentPerspective === 'all' || currentPerspective === 'human'
  const showGeek = currentPerspective === 'all' || currentPerspective === 'geek'
  const showAgent = currentPerspective === 'all' || currentPerspective === 'agent'

  return (
    <div className="grid gap-4 lg:grid-cols-3">
      {/* 1. HUMAIN & DÉCIDEUR */}
      {showHuman && perspectives.human && (
        <div className="rounded-2xl border border-emerald-500/30 bg-emerald-500/5 p-5 shadow-xs space-y-3">
          <div className="flex items-center gap-2">
            <div className="flex h-7 w-7 items-center justify-center rounded-lg bg-emerald-500 text-white">
              <User size={15} />
            </div>
            <div>
              <h4 className="text-xs font-black uppercase tracking-wider text-emerald-600 dark:text-emerald-400">
                Humain &amp; Décideur
              </h4>
              <p className="text-[11px] text-fg-muted font-medium">{perspectives.human.subtitle}</p>
            </div>
          </div>
          <div className="space-y-2.5 pt-2">
            {perspectives.human.points?.map((pt) => (
              <div key={pt.label} className="rounded-xl border border-line bg-surface/80 p-3">
                <span className="text-[10px] font-bold uppercase tracking-wider text-emerald-600 dark:text-emerald-400">
                  {pt.label}
                </span>
                <p className="mt-1 text-xs text-fg leading-relaxed">{pt.text}</p>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* 2. INGÉNIEUR SYSTÈME (GEEK) */}
      {showGeek && perspectives.geek && (
        <div className="rounded-2xl border border-sky-500/30 bg-sky-500/5 p-5 shadow-xs space-y-3">
          <div className="flex items-center gap-2">
            <div className="flex h-7 w-7 items-center justify-center rounded-lg bg-sky-500 text-white">
              <Cpu size={15} />
            </div>
            <div>
              <h4 className="text-xs font-black uppercase tracking-wider text-sky-600 dark:text-sky-400">
                Ingénieur Système (Geek)
              </h4>
              <p className="text-[11px] text-fg-muted font-medium">{perspectives.geek.subtitle}</p>
            </div>
          </div>
          <div className="space-y-2.5 pt-2">
            {perspectives.geek.points?.map((pt) => (
              <div key={pt.label} className="rounded-xl border border-line bg-surface/80 p-3">
                <span className="text-[10px] font-bold uppercase tracking-wider text-sky-600 dark:text-sky-400">
                  {pt.label}
                </span>
                <p className="mt-1 text-xs text-fg leading-relaxed font-mono">{pt.text}</p>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* 3. AGENT IA OPÉRATEUR */}
      {showAgent && perspectives.agent && (
        <div className="rounded-2xl border border-amber-500/30 bg-amber-500/5 p-5 shadow-xs space-y-3">
          <div className="flex items-center gap-2">
            <div className="flex h-7 w-7 items-center justify-center rounded-lg bg-amber-500 text-white">
              <Bot size={15} />
            </div>
            <div>
              <h4 className="text-xs font-black uppercase tracking-wider text-amber-600 dark:text-amber-400">
                Tandem Agent IA (Machine)
              </h4>
              <p className="text-[11px] text-fg-muted font-medium">{perspectives.agent.subtitle}</p>
            </div>
          </div>
          <div className="space-y-2.5 pt-2">
            {perspectives.agent.points?.map((pt) => (
              <div key={pt.label} className="rounded-xl border border-line bg-surface/80 p-3">
                <span className="text-[10px] font-bold uppercase tracking-wider text-amber-600 dark:text-amber-400">
                  {pt.label}
                </span>
                <p className="mt-1 text-xs text-fg leading-relaxed">{pt.text}</p>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  )
}
