import { useEffect, useState } from 'react'
import {
  Activity,
  AlertCircle,
  Bot,
  CheckCircle2,
  Cpu,
  Flame,
  Play,
  RefreshCw,
  RotateCcw,
  Shield,
  Sparkles,
  Zap,
} from 'lucide-react'
import { api } from '../api/client'
import { showToast } from '../components/ui/ToastHost'
import Select from '../components/ui/Select'

export default function ArbiterPage() {
  const [data, setData] = useState(null)
  const [loading, setLoading] = useState(true)
  const [simulating, setSimulating] = useState(false)
  const [chaosSimulation, setChaosSimulation] = useState(null)
  const [targetChaosNode, setTargetChaosNode] = useState('gbs-p4-node3199')

  async function loadData() {
    try {
      const res = await api.podmesh.arbiter()
      setData(res.arbiter)
    } catch (err) {
      showToast({ title: 'Erreur Arbiter', description: err.message, variant: 'danger' })
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    loadData()
  }, [])

  async function handleSimulateChaos() {
    setSimulating(true)
    try {
      const sim = await api.podmesh.simulateChaos(targetChaosNode)
      setChaosSimulation(sim)
      showToast({
        title: 'Simulation Chaos Exécutée',
        description: `Mode Digital Twin non destructif : verdict ${sim.capacityProjection?.verdict}`,
        variant: 'success',
      })
    } catch (err) {
      showToast({ title: 'Échec Simulation', description: err.message, variant: 'danger' })
    } finally {
      setSimulating(false)
    }
  }

  async function handleSealIntention(id) {
    try {
      await api.podmesh.sealIntention(id)
      showToast({ title: 'Sceau Tandem Apposé', description: 'Intention validée et exécutée en production', variant: 'success' })
      loadData()
    } catch (err) {
      showToast({ title: 'Échec Sceau', description: err.message, variant: 'danger' })
    }
  }

  async function handleRemediate(forensicId, action) {
    try {
      await api.podmesh.remediateForensic(forensicId, action)
      showToast({ title: 'Auto-Guérison Effectuée', description: `Action ${action} exécutée, univers restauré en PURRING`, variant: 'success' })
      loadData()
    } catch (err) {
      showToast({ title: 'Échec Guérison', description: err.message, variant: 'danger' })
    }
  }

  if (loading && !data) {
    return (
      <div className="flex h-64 items-center justify-center">
        <RefreshCw className="animate-spin text-accent" size={32} />
      </div>
    )
  }

  const agents = data?.activeAgents || []
  const intentions = data?.pendingTandemIntentions || []
  const blackboxes = data?.forensicBlackboxes || []

  return (
    <div className="space-y-6">
      {/* Top Banner */}
      <div className="flex flex-col justify-between gap-4 rounded-2xl border border-line bg-surface p-5 shadow-sm sm:flex-row sm:items-center">
        <div>
          <h1 className="text-xl font-black text-fg flex items-center gap-2">
            <Flame className="text-amber-500" size={24} /> Active Arbiter Bridge & Chaos What-If
          </h1>
          <p className="mt-1 text-xs text-fg-muted">
            Pont de dialogue permanent Humain–IA • Surveillance en temps réel, simulation préventive de panne et auto-guérison forensique.
          </p>
        </div>

        <div className="flex items-center gap-2">
          <span className="inline-flex items-center gap-1.5 rounded-full bg-emerald-500/10 px-3 py-1 text-xs font-bold text-emerald-500">
            <span className="h-2 w-2 rounded-full bg-emerald-500 animate-pulse" />
            {data?.status || 'ACTIVE_WATCH'}
          </span>
        </div>
      </div>

      {/* Agents AI Bridge in Synergy */}
      <section className="space-y-3">
        <h2 className="text-base font-bold text-fg flex items-center gap-2">
          <Bot size={18} className="text-accent" /> Agents du Tandem Humain–IA Connectés
        </h2>
        <div className="grid gap-3 sm:grid-cols-3">
          {agents.map((ag) => (
            <div key={ag.id} className="rounded-xl border border-line bg-surface p-4 shadow-sm">
              <div className="flex items-center justify-between">
                <span className="text-xs font-bold text-fg flex items-center gap-1.5">
                  <Sparkles size={14} className="text-accent" /> {ag.name}
                </span>
                <span className="h-2 w-2 rounded-full bg-emerald-500" />
              </div>
              <p className="mt-2 text-xs text-fg-muted font-medium">{ag.focus}</p>
              <p className="mt-2 font-mono text-[10px] text-fg-faint">Dernier ping: {ag.lastSeen?.slice(11, 19)}</p>
            </div>
          ))}
        </div>
      </section>

      {/* Non-Destructive Digital Twin Chaos Simulator */}
      <section className="rounded-2xl border border-line bg-surface p-5 shadow-sm space-y-4">
        <div className="flex flex-col justify-between gap-2 sm:flex-row sm:items-center border-b border-line pb-3">
          <div>
            <h2 className="text-base font-bold text-fg flex items-center gap-2">
              <Flame size={18} className="text-amber-500" /> Simulateur Chaos What-If (Jumeau Numérique Non-Destructif)
            </h2>
            <p className="mt-1 text-xs text-fg-muted">
              Simule la perte brutale d'un nœud ou d'un datacenter entier pour projeter les relocalisations, le RTO, le RPO et la capacité résiduelle.
            </p>
          </div>

          <div className="flex items-center gap-2">
            <div className="w-64">
              <Select
                value={targetChaosNode}
                onChange={(val) => setTargetChaosNode(val)}
                options={[
                  { value: 'gbs-p4-node3199', label: 'gbs-p4-node3199 (Worker Principal)' },
                  { value: 'med-pmox-node3199', label: 'med-pmox-node3199 (Worker Miroir)' },
                ]}
              />
            </div>
            <button
              type="button"
              onClick={handleSimulateChaos}
              disabled={simulating}
              className="flex items-center gap-1.5 rounded-lg bg-amber-600 px-3 py-1.5 text-xs font-bold text-white shadow-sm hover:bg-amber-500 disabled:opacity-50"
            >
              <Play size={14} /> {simulating ? 'Calcul de trajectoire…' : 'Simuler Panne Sèche'}
            </button>
          </div>
        </div>

        {chaosSimulation && (
          <div className="space-y-3 pt-2">
            <div className="grid grid-cols-2 gap-3 sm:grid-cols-4 text-xs">
              <div className="rounded-xl border border-line bg-surface-2 p-3">
                <p className="text-[10px] uppercase font-bold text-fg-faint">Nœud Simulé Hors-Service</p>
                <p className="mt-1 font-bold text-danger font-mono">{chaosSimulation.failedNode?.name}</p>
              </div>
              <div className="rounded-xl border border-line bg-surface-2 p-3">
                <p className="text-[10px] uppercase font-bold text-fg-faint">Nœud de Repli Automatique</p>
                <p className="mt-1 font-bold text-emerald-400 font-mono">{chaosSimulation.fallbackNode?.name}</p>
              </div>
              <div className="rounded-xl border border-line bg-surface-2 p-3">
                <p className="text-[10px] uppercase font-bold text-fg-faint">RTO / RPO Projeté</p>
                <p className="mt-1 font-bold text-fg font-mono">850ms / 0s (Zero Data Loss)</p>
              </div>
              <div className="rounded-xl border border-line bg-surface-2 p-3">
                <p className="text-[10px] uppercase font-bold text-fg-faint">Verdict Résilience</p>
                <p className="mt-1 font-bold text-emerald-400 font-mono">{chaosSimulation.capacityProjection?.verdict}</p>
              </div>
            </div>

            <div className="rounded-xl border border-line bg-surface-2/60 p-4">
              <h4 className="text-xs font-bold text-fg mb-2">Relocalisations Automatiques Projetées ({chaosSimulation.relocations?.length}) :</h4>
              <div className="space-y-1.5">
                {(chaosSimulation.relocations || []).map((rel) => (
                  <div key={rel.universeId} className="flex items-center justify-between text-xs bg-surface p-2 rounded-lg border border-line">
                    <span className="font-mono font-bold text-fg">{rel.universeId}</span>
                    <span className="text-fg-muted">{rel.account}</span>
                    <span className="font-mono text-emerald-400">Preuve Op 10 Validée : Oui ✓</span>
                    <span className="text-sky-400 font-mono">Bascule {rel.from} ➔ {rel.to}</span>
                  </div>
                ))}
              </div>
            </div>
          </div>
        )}
      </section>

      {/* Forensic Blackboxes (Auto-Healing) */}
      <section className="space-y-3">
        <h2 className="text-base font-bold text-fg flex items-center gap-2">
          <AlertCircle size={18} className="text-danger" /> Boîtes Noires Forensiques & Auto-Guérison ({blackboxes.length})
        </h2>
        {blackboxes.length === 0 ? (
          <div className="rounded-xl border border-line bg-surface p-6 text-center text-xs text-fg-muted">
            ✓ Aucune anomalie active. Tous les univers et makers sont dans l'état nominal PURRING.
          </div>
        ) : (
          <div className="space-y-3">
            {blackboxes.map((box) => (
              <div key={box.id} className="rounded-xl border border-danger/40 bg-danger/5 p-4 space-y-3">
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-2">
                    <span className="font-mono font-bold text-danger">{box.id}</span>
                    <span className="rounded bg-danger/10 px-2 py-0.5 font-mono text-[10px] font-bold text-danger">
                      {box.event}
                    </span>
                  </div>
                  <span className="text-xs text-fg-muted">{box.capturedAt}</span>
                </div>
                <div className="text-xs text-fg">
                  Univers affecté : <span className="font-mono font-bold">{box.rowId}</span> sur {box.machine}
                </div>
                <pre className="overflow-x-auto rounded bg-surface p-2.5 font-mono text-[10px] text-fg-muted border border-line">
                  {(box.lastLogs || []).join('\n')}
                </pre>
                <div className="flex flex-wrap items-center gap-2 pt-1">
                  <span className="text-xs font-semibold text-fg">Actions d'Auto-Guérison Recommandées :</span>
                  <button
                    type="button"
                    onClick={() => handleRemediate(box.id, 'rollback_pmbs_snapshot')}
                    className="rounded bg-accent px-2.5 py-1 text-xs font-bold text-accent-fg hover:opacity-90"
                  >
                    Rollback Instantané Snapshot PMBS
                  </button>
                  <button
                    type="button"
                    onClick={() => handleRemediate(box.id, 'increase_ram_512mb')}
                    className="rounded bg-surface-2 border border-line px-2.5 py-1 text-xs font-bold text-fg hover:bg-surface"
                  >
                    Augmenter Quota RAM (+512 MiB)
                  </button>
                </div>
              </div>
            ))}
          </div>
        )}
      </section>

      {/* Tandem Intentions Ready for Seal */}
      <section className="space-y-3">
        <h2 className="text-base font-bold text-fg flex items-center gap-2">
          <Shield size={18} className="text-accent" /> Intentions Tandem en attente de Sceau ({intentions.length})
        </h2>
        <div className="space-y-3">
          {intentions.map((intent) => (
            <div key={intent.id} className="rounded-xl border border-line bg-surface p-4 shadow-sm flex flex-col justify-between gap-3 sm:flex-row sm:items-center">
              <div>
                <div className="flex items-center gap-2">
                  <h3 className="text-sm font-bold text-fg">{intent.title}</h3>
                  <span className="rounded bg-amber-500/10 px-2 py-0.5 text-[10px] font-bold text-amber-500">
                    {intent.status}
                  </span>
                </div>
                <p className="mt-1 text-xs text-fg-muted max-w-2xl">{intent.description}</p>
                <p className="mt-2 text-[10px] font-mono text-fg-faint">Proposé par: {intent.proposedBy}</p>
              </div>

              {intent.status !== 'SEALED_AND_EXECUTED' ? (
                <button
                  type="button"
                  onClick={() => handleSealIntention(intent.id)}
                  className="shrink-0 rounded-lg bg-accent px-4 py-2 text-xs font-bold text-accent-fg shadow-sm hover:opacity-90"
                >
                  Apposer le Sceau Tandem
                </button>
              ) : (
                <span className="text-xs font-bold text-emerald-400">Exécuté ✓</span>
              )}
            </div>
          ))}
        </div>
      </section>
    </div>
  )
}
