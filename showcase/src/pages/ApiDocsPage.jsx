import { useEffect, useState } from 'react'
import {
  BookOpen,
  Code2,
  ExternalLink,
  FileCode,
  Layers,
  RefreshCw,
  Sparkles,
  Terminal,
} from 'lucide-react'
import { api } from '../api/client'
import { showToast } from '../components/ui/ToastHost'

export default function ApiDocsPage() {
  const [protocol, setProtocol] = useState(null)
  const [loading, setLoading] = useState(true)

  async function loadData() {
    try {
      const proto = await api.podmesh.protocolSpec()
      setProtocol(proto)
    } catch (err) {
      showToast({ title: 'Erreur Docs', description: err.message, variant: 'danger' })
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    loadData()
  }, [])

  if (loading && !protocol) {
    return (
      <div className="flex h-64 items-center justify-center">
        <RefreshCw className="animate-spin text-accent" size={32} />
      </div>
    )
  }

  return (
    <div className="space-y-6">
      {/* Top Banner */}
      <div className="flex flex-col justify-between gap-4 rounded-2xl border border-line bg-surface p-5 shadow-sm sm:flex-row sm:items-center">
        <div>
          <h1 className="text-xl font-black text-fg flex items-center gap-2">
            <BookOpen className="text-accent" size={24} /> Documentation & Spécifications API 100% "API-Sable"
          </h1>
          <p className="mt-1 text-xs text-fg-muted">
            Toutes les fonctionnalités de PodMesh sont pilotables par API REST, OpenAPI 3.1, et briefing pour agents IA (Claude Code, ChatGPT, Antigravity).
          </p>
        </div>

        <div className="flex items-center gap-2">
          <a
            href="/api/docs"
            target="_blank"
            rel="noreferrer"
            className="flex items-center gap-1.5 rounded-lg bg-accent px-3 py-1.5 text-xs font-bold text-accent-fg shadow-sm hover:opacity-90"
          >
            <ExternalLink size={14} /> Ouvrir Documentation OpenAPI (Swagger)
          </a>
        </div>
      </div>

      {/* Quick AI & API Links Grid */}
      <div className="grid gap-4 sm:grid-cols-3">
        <a
          href="/api/openapi.json"
          target="_blank"
          rel="noreferrer"
          className="rounded-xl border border-line bg-surface p-4 shadow-sm hover:border-accent transition-all group"
        >
          <div className="flex items-center justify-between">
            <FileCode size={20} className="text-accent" />
            <ExternalLink size={14} className="text-fg-muted group-hover:text-accent" />
          </div>
          <h3 className="mt-2 text-sm font-bold text-fg">Spécification OpenAPI 3.1</h3>
          <p className="mt-1 text-xs text-fg-muted">
            Format JSON standard machine-readable pour génération de clients et import Swagger/Postman.
          </p>
          <span className="mt-2 inline-block font-mono text-[11px] text-accent font-semibold">GET /api/openapi.json</span>
        </a>

        <a
          href="/api/v1/ai-briefing"
          target="_blank"
          rel="noreferrer"
          className="rounded-xl border border-line bg-surface p-4 shadow-sm hover:border-accent transition-all group"
        >
          <div className="flex items-center justify-between">
            <Sparkles size={20} className="text-sky-400" />
            <ExternalLink size={14} className="text-fg-muted group-hover:text-sky-400" />
          </div>
          <h3 className="mt-2 text-sm font-bold text-fg">Briefing Instantané IA</h3>
          <p className="mt-1 text-xs text-fg-muted">
            Synthèse condensée de l'état du cluster, sites, quorum, univers et invariants pour agents autonomes.
          </p>
          <span className="mt-2 inline-block font-mono text-[11px] text-sky-400 font-semibold">GET /api/v1/ai-briefing</span>
        </a>

        <a
          href="/api/v1/status.json"
          target="_blank"
          rel="noreferrer"
          className="rounded-xl border border-line bg-surface p-4 shadow-sm hover:border-accent transition-all group"
        >
          <div className="flex items-center justify-between">
            <Code2 size={20} className="text-emerald-400" />
            <ExternalLink size={14} className="text-fg-muted group-hover:text-emerald-400" />
          </div>
          <h3 className="mt-2 text-sm font-bold text-fg">Statut Canonique status.json</h3>
          <p className="mt-1 text-xs text-fg-muted">
            Format canonique SHAPER OS v1.15 : état PURRING, horodatages de heartbeat et époque de quorum.
          </p>
          <span className="mt-2 inline-block font-mono text-[11px] text-emerald-400 font-semibold">GET /api/v1/status.json</span>
        </a>
      </div>

      {/* Protocol Contract Markdown Viewer */}
      <section className="space-y-3">
        <div className="flex items-center justify-between">
          <h2 className="text-base font-bold text-fg flex items-center gap-2">
            <Terminal size={18} className="text-accent" /> Contrat Formel SHAPER OS v1.15 Governor & Maker
          </h2>
          <span className="rounded bg-surface-2 px-2 py-0.5 font-mono text-[11px] text-fg-muted">
            {protocol?.invariantsCount || 12} Invariants Scellés
          </span>
        </div>

        <div className="rounded-2xl border border-line bg-surface p-5 shadow-sm space-y-4">
          <div className="grid grid-cols-2 gap-3 sm:grid-cols-4 text-xs border-b border-line pb-4">
            <div>
              <p className="text-[10px] uppercase font-bold text-fg-faint">Work Kinds</p>
              <p className="mt-1 font-mono text-fg font-bold">{(protocol?.workKinds || []).join(', ')}</p>
            </div>
            <div>
              <p className="text-[10px] uppercase font-bold text-fg-faint">Ledger States</p>
              <p className="mt-1 font-mono text-fg font-bold">{(protocol?.ledgerStates || []).join(', ')}</p>
            </div>
            <div>
              <p className="text-[10px] uppercase font-bold text-fg-faint">Maker Host Kinds</p>
              <p className="mt-1 font-mono text-fg font-bold">{(protocol?.makerHostKinds || []).join(', ')}</p>
            </div>
            <div>
              <p className="text-[10px] uppercase font-bold text-fg-faint">Version Protocole</p>
              <p className="mt-1 font-mono text-emerald-400 font-bold">{protocol?.protocolVersion}</p>
            </div>
          </div>

          <pre className="overflow-x-auto rounded-xl bg-surface-2/60 p-4 font-mono text-xs text-fg-muted leading-relaxed whitespace-pre-wrap max-h-[500px]">
            {protocol?.markdown}
          </pre>
        </div>
      </section>
    </div>
  )
}
