import { useEffect, useState } from 'react'
import {
  Boxes,
  CheckCircle2,
  Clock,
  Database,
  Layers,
  Play,
  Plus,
  RefreshCw,
  Server,
  Shield,
  Terminal,
} from 'lucide-react'
import { api } from '../api/client'
import { showToast } from '../components/ui/ToastHost'
import Select from '../components/ui/Select'

export default function FractalsPage() {
  const [data, setData] = useState(null)
  const [loading, setLoading] = useState(true)
  const [desireModalOpen, setDesireModalOpen] = useState(false)
  const [runningCycle, setRunningCycle] = useState(false)
  const [cycleResult, setCycleResult] = useState(null)
  const [newDesire, setNewDesire] = useState({
    governorId: 'gov-saas-demo',
    account: '',
    klass: 'boutique_woocommerce',
    machine: 'gbs-p4-node3199',
    env: 'demo',
    client_slug: '',
    shop_domain: '',
    quota_ram_mb: 512,
  })

  async function loadData() {
    try {
      const res = await api.podmesh.fractals()
      setData(res)
    } catch (err) {
      showToast({ title: 'Erreur Fractales', description: err.message, variant: 'danger' })
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    loadData()
  }, [])

  async function handleDesireSubmit(e) {
    e.preventDefault()
    try {
      const payload = {
        governorId: newDesire.governorId,
        account: newDesire.account,
        klass: newDesire.klass,
        machine: newDesire.machine,
        env: newDesire.env,
        params: {
          client_slug: newDesire.client_slug || newDesire.account.replace(/[^a-z0-9_]/gi, '_').toLowerCase(),
          shop_domain: newDesire.shop_domain || `${newDesire.account}.xavdp.pro`,
          quota_ram_mb: Number(newDesire.quota_ram_mb) || 512,
        },
      }
      const res = await api.podmesh.desireRow(payload)
      showToast({ title: 'Intention DESIRED écrite', description: `Ligne de ledger ${res.row.id} créée`, variant: 'success' })
      setDesireModalOpen(false)
      loadData()
    } catch (err) {
      showToast({ title: 'Échec DESIRED', description: err.message, variant: 'danger' })
    }
  }

  async function handleRunSpecializerCycle() {
    setRunningCycle(true)
    try {
      const res = await api.podmesh.runSpecializerCycle({ host: 'gbs-p4-node3199' })
      setCycleResult(res)
      showToast({ title: 'Cycle Maker Exécuté', description: `${res.results?.length || 0} univers réconciliés et spécialisés!`, variant: 'success' })
      loadData()
    } catch (err) {
      showToast({ title: 'Échec Cycle', description: err.message, variant: 'danger' })
    } finally {
      setRunningCycle(false)
    }
  }

  if (loading && !data) {
    return (
      <div className="flex h-64 items-center justify-center">
        <RefreshCw className="animate-spin text-accent" size={32} />
      </div>
    )
  }

  const governors = data?.fractalGovernors || []
  const makers = data?.makers || []
  const ledgerRows = data?.ledgerRows || []

  return (
    <div className="space-y-6">
      {/* Top Banner */}
      <div className="flex flex-col justify-between gap-4 rounded-2xl border border-line bg-surface p-5 shadow-sm sm:flex-row sm:items-center">
        <div>
          <h1 className="text-xl font-black text-fg flex items-center gap-2">
            <Layers className="text-accent" size={24} /> Gouverneurs de Fractales & Makers SHAPER OS v1.15
          </h1>
          <p className="mt-1 text-xs text-fg-muted">
            Strate 1 : Gouvernance applicative par écriture dans le Ledger privé MariaDB • Strate 2 : Exécution outbound par les Makers
          </p>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <button
            type="button"
            onClick={handleRunSpecializerCycle}
            disabled={runningCycle}
            className="flex items-center gap-1.5 rounded-lg border border-accent bg-accent/10 px-3 py-1.5 text-xs font-bold text-accent hover:bg-accent/20"
          >
            <Play size={14} /> {runningCycle ? 'Cycle en cours…' : 'Déclencher Cycle Maker'}
          </button>
          <button
            type="button"
            onClick={() => setDesireModalOpen(true)}
            className="flex items-center gap-1.5 rounded-lg bg-accent px-3 py-1.5 text-xs font-bold text-accent-fg shadow-sm hover:opacity-90"
          >
            <Plus size={15} /> Écrire Intention DESIRED
          </button>
        </div>
      </div>

      {/* Cycle Results Banner (if recently triggered) */}
      {cycleResult && (
        <div className="rounded-xl border border-emerald-500/30 bg-emerald-500/10 p-4 text-xs">
          <div className="flex items-center justify-between">
            <span className="font-bold text-emerald-400 flex items-center gap-1.5">
              <CheckCircle2 size={16} /> Résultat du Cycle Spécialiseur Maker (host: {cycleResult.host})
            </span>
            <span className="font-mono text-emerald-400">polled: {cycleResult.polledWorkCount}</span>
          </div>
          <pre className="mt-2 overflow-x-auto rounded bg-surface p-2 font-mono text-[11px] text-fg-muted border border-line">
            {JSON.stringify(cycleResult.results || [], null, 2)}
          </pre>
        </div>
      )}

      {/* Strate 1: Fractal Governors List */}
      <section className="space-y-3">
        <h2 className="text-base font-bold text-fg flex items-center gap-2">
          <Database size={18} className="text-sky-400" /> Gouverneurs SaaS Actifs (Strate 1)
        </h2>
        <div className="grid gap-4 md:grid-cols-2">
          {governors.map((gov) => (
            <div key={gov.id} className="rounded-xl border border-line bg-surface p-4 shadow-sm">
              <div className="flex items-start justify-between">
                <div>
                  <h3 className="text-sm font-bold text-fg">{gov.name}</h3>
                  <p className="font-mono text-xs text-sky-400">{gov.universeId}</p>
                </div>
                <span className="rounded-full bg-emerald-500/10 px-2 py-0.5 text-xs font-bold text-emerald-500">
                  {gov.state}
                </span>
              </div>
              <div className="mt-3 space-y-1.5 text-xs">
                <div className="flex justify-between text-fg-muted">
                  <span>Ledger Dédié:</span>
                  <span className="font-mono text-fg">{gov.ledgerDb}</span>
                </div>
                <div className="flex justify-between text-fg-muted">
                  <span>Nœud d'Hébergement:</span>
                  <span className="font-mono text-fg">{gov.nodeId} ({gov.site})</span>
                </div>
                <div className="flex justify-between text-fg-muted">
                  <span>Makers Enrôlés:</span>
                  <span className="font-mono text-fg">{(gov.enrolledMakers || []).join(', ')}</span>
                </div>
              </div>
            </div>
          ))}
        </div>
      </section>

      {/* Strate 2: Enrolled Makers */}
      <section className="space-y-3">
        <h2 className="text-base font-bold text-fg flex items-center gap-2">
          <Terminal size={18} className="text-indigo-400" /> Makers Enrôlés (Strate 2 — Outbound Polling)
        </h2>
        <div className="grid gap-3 md:grid-cols-2">
          {makers.map((mk) => (
            <div key={mk.id} className="rounded-xl border border-line bg-surface p-4 shadow-sm">
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2">
                  <Server size={16} className="text-indigo-400" />
                  <h3 className="font-mono text-sm font-bold text-fg">{mk.host}</h3>
                </div>
                <span className="rounded bg-indigo-500/10 px-2 py-0.5 font-mono text-[11px] text-indigo-400">
                  {mk.lanes} lanes parallèles
                </span>
              </div>
              <p className="mt-1 text-xs text-fg-muted">Site {mk.site} • Version {mk.version}</p>
              <div className="mt-3">
                <p className="text-[11px] font-semibold text-fg-muted">Digests SHA-256 Vérifiés sur l'Hôte :</p>
                <div className="mt-1 space-y-1">
                  {(mk.inventoryDigests || []).map((dig) => (
                    <div key={dig} className="font-mono text-[10px] text-fg-muted bg-surface-2 p-1 rounded truncate">
                      {dig}
                    </div>
                  ))}
                </div>
              </div>
            </div>
          ))}
        </div>
      </section>

      {/* Desired-State Ledger Rows Table */}
      <section className="space-y-3">
        <h2 className="text-base font-bold text-fg flex items-center gap-2">
          <Clock size={18} className="text-accent" /> Registre d'État Désiré (Ledger Rows)
        </h2>
        <div className="overflow-x-auto rounded-xl border border-line bg-surface shadow-sm">
          <table className="w-full text-left text-xs">
            <thead className="border-b border-line bg-surface-2/60 text-fg-muted uppercase tracking-wider text-[10px]">
              <tr>
                <th className="px-4 py-3">Ligne / ID</th>
                <th className="px-3 py-3">Gouverneur</th>
                <th className="px-3 py-3">Compte / Client</th>
                <th className="px-3 py-3">Machine Cible</th>
                <th className="px-3 py-3">État Ledger</th>
                <th className="px-3 py-3">Dernier Événement</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-line/60">
              {ledgerRows.map((r) => {
                const lastEvent = r.events?.[r.events.length - 1]
                return (
                  <tr key={r.id} className="hover:bg-surface-2/40 transition-colors">
                    <td className="px-4 py-3 font-mono font-bold text-fg">{r.id}</td>
                    <td className="px-3 py-3 font-mono text-sky-400">{r.governorId}</td>
                    <td className="px-3 py-3 font-medium text-fg">{r.account}</td>
                    <td className="px-3 py-3 font-mono text-fg-muted">{r.machine}</td>
                    <td className="px-3 py-3">
                      <span
                        className={`inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[10px] font-bold ${
                          r.state === 'PURRING'
                            ? 'bg-emerald-500/10 text-emerald-500'
                            : r.state === 'DESIRED'
                            ? 'bg-amber-500/10 text-amber-500'
                            : 'bg-sky-500/10 text-sky-500'
                        }`}
                      >
                        {r.state}
                      </span>
                    </td>
                    <td className="px-3 py-3 font-mono text-[11px] text-fg-muted">
                      {lastEvent ? `${lastEvent.event} (${lastEvent.maker || 'system'})` : 'none'}
                    </td>
                  </tr>
                )
              })}
            </tbody>
          </table>
        </div>
      </section>

      {/* Modal Écriture Intention DESIRED */}
      {desireModalOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4 backdrop-blur-xs">
          <div className="w-full max-w-lg rounded-2xl border border-line bg-surface p-6 shadow-2xl">
            <h3 className="text-lg font-bold text-fg">Écrire une intention DESIRED dans le Ledger</h3>
            <p className="mt-1 text-xs text-fg-muted">
              Le Gouverneur inscrit l'intention. Le Maker spécialisera l'univers lors de son prochain cycle de polling.
            </p>
            <form onSubmit={handleDesireSubmit} className="mt-4 space-y-3 text-xs">
              <div>
                <label className="block font-semibold text-fg mb-1">Gouverneur Cible</label>
                <Select
                  value={newDesire.governorId}
                  onChange={(val) => setNewDesire({ ...newDesire, governorId: val })}
                  options={governors.map((g) => ({ value: g.id, label: `${g.name} (${g.id})` }))}
                />
              </div>
              <div>
                <label className="block font-semibold text-fg mb-1">Compte Client (Account)</label>
                <input
                  type="text"
                  required
                  placeholder="ex: acct-librairie-paris"
                  value={newDesire.account}
                  onChange={(e) => setNewDesire({ ...newDesire, account: e.target.value })}
                  className="w-full rounded-lg border border-line bg-surface-2 px-3 py-2 text-fg focus:border-accent focus:outline-hidden"
                />
              </div>
              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className="block font-semibold text-fg mb-1">Classe de Spécialisation</label>
                  <input
                    type="text"
                    value={newDesire.klass}
                    onChange={(e) => setNewDesire({ ...newDesire, klass: e.target.value })}
                    className="w-full rounded-lg border border-line bg-surface-2 px-3 py-2 text-fg focus:border-accent focus:outline-hidden"
                  />
                </div>
                <div>
                  <label className="block font-semibold text-fg mb-1">Machine d'Exécution</label>
                  <Select
                    value={newDesire.machine}
                    onChange={(val) => setNewDesire({ ...newDesire, machine: val })}
                    options={[
                      { value: 'gbs-p4-node3199', label: 'gbs-p4-node3199' },
                      { value: 'med-pmox-node3199', label: 'med-pmox-node3199' },
                    ]}
                  />
                </div>
              </div>
              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className="block font-semibold text-fg mb-1">Client Slug (^[a-z][a-z0-9_]*$)</label>
                  <input
                    type="text"
                    placeholder="librairie_paris"
                    value={newDesire.client_slug}
                    onChange={(e) => setNewDesire({ ...newDesire, client_slug: e.target.value })}
                    className="w-full rounded-lg border border-line bg-surface-2 px-3 py-2 font-mono text-fg focus:border-accent focus:outline-hidden"
                  />
                </div>
                <div>
                  <label className="block font-semibold text-fg mb-1">Quota RAM (MiB)</label>
                  <input
                    type="number"
                    value={newDesire.quota_ram_mb}
                    onChange={(e) => setNewDesire({ ...newDesire, quota_ram_mb: e.target.value })}
                    className="w-full rounded-lg border border-line bg-surface-2 px-3 py-2 text-fg focus:border-accent focus:outline-hidden"
                  />
                </div>
              </div>

              <div className="flex items-center justify-end gap-2 pt-3 border-t border-line">
                <button
                  type="button"
                  onClick={() => setDesireModalOpen(false)}
                  className="rounded-lg border border-line px-4 py-2 font-medium text-fg hover:bg-surface-2"
                >
                  Annuler
                </button>
                <button
                  type="submit"
                  className="rounded-lg bg-accent px-4 py-2 font-bold text-accent-fg shadow-sm hover:opacity-90"
                >
                  Écrire dans le Ledger MariaDB
                </button>
              </div>
            </form>
          </div>
        </div>
      )}
    </div>
  )
}
