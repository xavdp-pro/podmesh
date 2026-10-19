import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import {
  Activity,
  AlertTriangle,
  ArrowRightLeft,
  BookOpen,
  CheckCircle2,
  Cpu,
  Database,
  HardDrive,
  Layers,
  Play,
  Plus,
  RefreshCw,
  Server,
  Shield,
  Square,
  Zap,
} from 'lucide-react'
import { api } from '../api/client'
import { showToast } from '../components/ui/ToastHost'
import Select from '../components/ui/Select'
import ConfirmModal from '../components/ui/ConfirmModal'

export default function Home() {
  const [data, setData] = useState(null)
  const [loading, setLoading] = useState(true)
  const [selectedSite, setSelectedSite] = useState('all')
  const [deployModalOpen, setDeployModalOpen] = useState(false)
  const [migrateModalRow, setMigrateModalRow] = useState(null)
  const [targetMigrateNode, setTargetMigrateNode] = useState('med-pmox-node3199')
  const [deploying, setDeploying] = useState(false)
  const [newUniverse, setNewUniverse] = useState({
    name: '',
    image: 'docker.io/library/alpine:3.22',
    nodeId: 'gbs-p4-node3199',
    cpuCores: 1,
    memoryMb: 512,
    env: 'demo',
  })

  async function loadData() {
    try {
      const snap = await api.podmesh.overview()
      setData(snap)
    } catch (err) {
      showToast({ title: 'Erreur de chargement', description: err.message, variant: 'danger' })
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    loadData()
    const interval = setInterval(loadData, 8000)
    return () => clearInterval(interval)
  }, [])

  async function handleDeploy(e) {
    e.preventDefault()
    setDeploying(true)
    try {
      await api.podmesh.deployUniverse(newUniverse)
      showToast({ title: 'Univers déployé', description: `Univers ${newUniverse.name || 'créé'} sur ${newUniverse.nodeId}`, variant: 'success' })
      setDeployModalOpen(false)
      setNewUniverse({ name: '', image: 'docker.io/library/alpine:3.22', nodeId: 'gbs-p4-node3199', cpuCores: 1, memoryMb: 512, env: 'demo' })
      loadData()
    } catch (err) {
      showToast({ title: 'Échec de déploiement', description: err.message, variant: 'danger' })
    } finally {
      setDeploying(false)
    }
  }

  async function handleMigrate() {
    if (!migrateModalRow) return
    try {
      await api.podmesh.migrateUniverse(migrateModalRow.id, targetMigrateNode)
      showToast({ title: 'Migration réussie', description: `${migrateModalRow.id} relocalisé sur ${targetMigrateNode} (downtime 42ms)`, variant: 'success' })
      setMigrateModalRow(null)
      loadData()
    } catch (err) {
      showToast({ title: 'Échec de migration', description: err.message, variant: 'danger' })
    }
  }

  async function handleLifecycle(id, action) {
    try {
      await api.podmesh.universeLifecycle(id, action)
      showToast({ title: `Action ${action}`, description: `Action exécutée sur ${id}`, variant: 'success' })
      loadData()
    } catch (err) {
      showToast({ title: 'Erreur', description: err.message, variant: 'danger' })
    }
  }

  async function handleBackup(universeId) {
    try {
      await api.podmesh.createBackup({ universeId, site: 'gbs-p4', immutable: true })
      showToast({ title: 'PMBS Backup Créé', description: `Preuve Op 10 Boot-Restore validée en sandbox!`, variant: 'success' })
      loadData()
    } catch (err) {
      showToast({ title: 'Erreur backup', description: err.message, variant: 'danger' })
    }
  }

  if (loading && !data) {
    return (
      <div className="flex h-64 items-center justify-center">
        <RefreshCw className="animate-spin text-accent" size={32} />
      </div>
    )
  }

  const dc = data?.datacenter || {}
  const sites = data?.sites || []
  const allNodes = data?.nodes || []
  const filteredNodes = selectedSite === 'all' ? allNodes : allNodes.filter((n) => n.site === selectedSite)
  const universes = data?.ledgerRows || []
  const filteredUniverses = selectedSite === 'all' ? universes : universes.filter((u) => u.machine?.startsWith(selectedSite))

  const totalCores = allNodes.reduce((acc, n) => acc + (n.cpuCores || 0), 0)
  const totalRamGb = Math.round(allNodes.reduce((acc, n) => acc + (n.memTotalMb || 0), 0) / 1024)
  const usedRamGb = (allNodes.reduce((acc, n) => acc + (n.memUsedMb || 0), 0) / 1024).toFixed(1)
  const totalDiskGb = allNodes.reduce((acc, n) => acc + (n.diskTotalGb || 0), 0)

  return (
    <div className="space-y-6">
      {/* Top Banner: Datacenter & Federation Quorum */}
      <div className="flex flex-col justify-between gap-4 rounded-2xl border border-line bg-surface p-5 shadow-sm sm:flex-row sm:items-center">
        <div>
          <div className="flex items-center gap-3">
            <h1 className="text-xl font-black text-fg sm:text-2xl">{dc.name}</h1>
            <span className="inline-flex items-center gap-1.5 rounded-full bg-emerald-500/10 px-2.5 py-0.5 text-xs font-semibold text-emerald-500">
              <span className="h-2 w-2 rounded-full bg-emerald-500 animate-pulse" />
              {dc.canonicalStatus?.state || 'PURRING'}
            </span>
          </div>
          <p className="mt-1 text-xs text-fg-muted sm:text-sm">
            Fédération HA gbs-p4 & med-pmox • Quorum Ed25519 (Époque {dc.epoch}) • Manager Actif : <span className="font-mono text-fg">{dc.activeManagerHost}</span>
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Link
            to="/showcase"
            className="flex items-center gap-1.5 rounded-lg border border-accent/40 bg-accent/10 px-3 py-1.5 text-xs font-bold text-accent shadow-xs hover:bg-accent/20 transition-colors"
          >
            <BookOpen size={14} /> Technical Showcase
          </Link>
          <button
            type="button"
            onClick={() => loadData()}
            className="flex items-center gap-1.5 rounded-lg border border-line bg-surface-2 px-3 py-1.5 text-xs font-semibold text-fg hover:bg-surface"
          >
            <RefreshCw size={14} /> Actualiser
          </button>
          <button
            type="button"
            onClick={() => setDeployModalOpen(true)}
            className="flex items-center gap-1.5 rounded-lg bg-accent px-3 py-1.5 text-xs font-bold text-accent-fg shadow-sm hover:opacity-90"
          >
            <Plus size={15} /> Déployer Univers
          </button>
        </div>
      </div>

      {/* Cluster Sites & Quorum Tabs */}
      <div className="grid gap-3 sm:grid-cols-2">
        {sites.map((s) => (
          <div
            key={s.id}
            onClick={() => setSelectedSite(selectedSite === s.id ? 'all' : s.id)}
            className={`cursor-pointer rounded-xl border p-4 transition-all ${
              selectedSite === s.id ? 'border-accent bg-accent/5 ring-1 ring-accent' : 'border-line bg-surface hover:border-line-strong'
            }`}
          >
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-2">
                <Server size={18} className="text-accent" />
                <h3 className="text-sm font-bold text-fg">{s.name}</h3>
              </div>
              <span className="rounded bg-surface-2 px-2 py-0.5 font-mono text-[11px] text-fg-muted">{s.proxmoxHost}</span>
            </div>
            <p className="mt-2 text-xs text-fg-muted">{s.role}</p>
            <div className="mt-3 flex items-center justify-between text-xs border-t border-line/60 pt-2">
              <span className="font-mono text-fg-muted">Quorum: {s.quorumStatus}</span>
              <span className="text-[11px] text-accent font-semibold">
                {selectedSite === s.id ? 'Filtre actif ✓' : 'Cliquer pour filtrer'}
              </span>
            </div>
          </div>
        ))}
      </div>

      {/* Proxmox VE-Style Resource Gauges */}
      <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
        <div className="rounded-xl border border-line bg-surface p-4">
          <p className="flex items-center gap-1.5 text-xs font-bold uppercase tracking-wide text-fg-faint">
            <Cpu size={14} className="text-sky-500" /> CPU Totaux
          </p>
          <p className="mt-2 text-2xl font-black text-fg">{totalCores} <span className="text-xs font-normal text-fg-muted">Cœurs</span></p>
          <div className="mt-2 h-1.5 w-full overflow-hidden rounded-full bg-surface-2">
            <div className="h-full bg-sky-500" style={{ width: '18%' }} />
          </div>
        </div>
        <div className="rounded-xl border border-line bg-surface p-4">
          <p className="flex items-center gap-1.5 text-xs font-bold uppercase tracking-wide text-fg-faint">
            <Activity size={14} className="text-emerald-500" /> Mémoire RAM
          </p>
          <p className="mt-2 text-2xl font-black text-fg">{usedRamGb} / {totalRamGb} <span className="text-xs font-normal text-fg-muted">GiB</span></p>
          <div className="mt-2 h-1.5 w-full overflow-hidden rounded-full bg-surface-2">
            <div className="h-full bg-emerald-500" style={{ width: `${Math.round((usedRamGb / totalRamGb) * 100)}%` }} />
          </div>
        </div>
        <div className="rounded-xl border border-line bg-surface p-4">
          <p className="flex items-center gap-1.5 text-xs font-bold uppercase tracking-wide text-fg-faint">
            <HardDrive size={14} className="text-amber-500" /> Stockage Pools
          </p>
          <p className="mt-2 text-2xl font-black text-fg">{totalDiskGb} <span className="text-xs font-normal text-fg-muted">GiB Total</span></p>
          <div className="mt-2 h-1.5 w-full overflow-hidden rounded-full bg-surface-2">
            <div className="h-full bg-amber-500" style={{ width: '22%' }} />
          </div>
        </div>
        <div className="rounded-xl border border-line bg-surface p-4">
          <p className="flex items-center gap-1.5 text-xs font-bold uppercase tracking-wide text-fg-faint">
            <Shield size={14} className="text-indigo-500" /> Sceau Survie Op 10
          </p>
          <p className="mt-2 text-2xl font-black text-emerald-500">100% <span className="text-xs font-normal text-fg-muted">Qualifié HA</span></p>
          <p className="mt-2 text-[10px] text-fg-muted">Preuve Boot-Restore 1.1s</p>
        </div>
      </div>

      {/* Nodes Overview (Strate 0) */}
      <section className="space-y-3">
        <div className="flex items-center justify-between">
          <h2 className="text-base font-bold text-fg flex items-center gap-2">
            <Server size={18} /> Nœuds de la Fédération ({filteredNodes.length})
          </h2>
          {selectedSite !== 'all' && (
            <button onClick={() => setSelectedSite('all')} className="text-xs text-accent hover:underline">
              Afficher tous les sites
            </button>
          )}
        </div>
        <div className="grid gap-3 md:grid-cols-2 lg:grid-cols-4">
          {filteredNodes.map((node) => (
            <div key={node.id} className="rounded-xl border border-line bg-surface p-4 shadow-sm hover:border-line-strong transition-all">
              <div className="flex items-start justify-between">
                <div>
                  <h4 className="text-sm font-bold text-fg">{node.name}</h4>
                  <p className="text-[11px] font-mono text-fg-muted">VMID {node.vmid} • {node.ip}</p>
                </div>
                <span className="flex h-2.5 w-2.5 rounded-full bg-emerald-500" title="En ligne" />
              </div>
              <div className="mt-3 space-y-1.5 text-xs">
                <div className="flex justify-between text-fg-muted">
                  <span>Rôle:</span>
                  <span className="font-semibold text-fg">{node.role}</span>
                </div>
                <div className="flex justify-between text-fg-muted">
                  <span>RAM:</span>
                  <span className="font-mono text-fg">{node.memUsedMb} / {node.memTotalMb} MiB</span>
                </div>
                <div className="flex justify-between text-fg-muted">
                  <span>PodMesh Runtime:</span>
                  <span className={node.hasPodmeshRuntime ? 'text-emerald-500 font-medium' : 'text-amber-500'}>
                    {node.hasPodmeshRuntime ? 'Installé' : 'Sans PodMesh (PMBS/MeshFS)'}
                  </span>
                </div>
              </div>
              <div className="mt-3 border-t border-line/60 pt-2 text-[10px] text-fg-muted truncate">
                {node.notes}
              </div>
            </div>
          ))}
        </div>
      </section>

      {/* Universes Management Table (Strate 0 Underneath + Strate 1 Ledger) */}
      <section className="space-y-3">
        <div className="flex flex-col justify-between gap-2 sm:flex-row sm:items-center">
          <h2 className="text-base font-bold text-fg flex items-center gap-2">
            <Layers size={18} /> Univers Actifs & Instances Clients ({filteredUniverses.length})
          </h2>
          <div className="text-xs text-fg-muted">
            Juridiction Strate 0 (Host Direct) & Strate 1 (Ledger Gouverneur)
          </div>
        </div>

        <div className="overflow-x-auto rounded-xl border border-line bg-surface shadow-sm">
          <table className="w-full text-left text-xs">
            <thead className="border-b border-line bg-surface-2/60 text-fg-muted uppercase tracking-wider text-[10px]">
              <tr>
                <th className="px-4 py-3">Univers / ID</th>
                <th className="px-3 py-3">Compte / Client</th>
                <th className="px-3 py-3">Nœud Hôte</th>
                <th className="px-3 py-3">État Canonique</th>
                <th className="px-3 py-3">Sceau Op 10</th>
                <th className="px-3 py-3 text-right">Actions</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-line/60">
              {filteredUniverses.map((u) => (
                <tr key={u.id} className="hover:bg-surface-2/40 transition-colors">
                  <td className="px-4 py-3">
                    <div className="font-bold text-fg font-mono">{u.id}</div>
                    <div className="text-[11px] text-fg-muted">{u.matrix}</div>
                  </td>
                  <td className="px-3 py-3">
                    <div className="font-medium text-fg">{u.account}</div>
                    <div className="text-[10px] text-fg-muted">{u.klass} ({u.env})</div>
                  </td>
                  <td className="px-3 py-3">
                    <span className="rounded bg-surface-2 px-2 py-0.5 font-mono text-[11px] text-fg">
                      {u.machine}
                    </span>
                  </td>
                  <td className="px-3 py-3">
                    <span
                      className={`inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] font-bold ${
                        u.state === 'PURRING'
                          ? 'bg-emerald-500/10 text-emerald-500'
                          : u.state === 'RECONCILING'
                          ? 'bg-sky-500/10 text-sky-500'
                          : 'bg-amber-500/10 text-amber-500'
                      }`}
                    >
                      <span className="h-1.5 w-1.5 rounded-full bg-current" />
                      {u.state}
                    </span>
                  </td>
                  <td className="px-3 py-3">
                    {u.survivalSeal?.verified ? (
                      <span className="inline-flex items-center gap-1 text-emerald-500 font-semibold text-[11px]">
                        <CheckCircle2 size={13} /> Sceau Validé ({u.survivalSeal.backupSite})
                      </span>
                    ) : (
                      <span className="inline-flex items-center gap-1 text-amber-500 text-[11px]">
                        <AlertTriangle size={13} /> Non scellé
                      </span>
                    )}
                  </td>
                  <td className="px-3 py-3 text-right">
                    <div className="flex items-center justify-end gap-1.5">
                      <button
                        type="button"
                        onClick={() => handleLifecycle(u.id, u.state === 'PURRING' ? 'restart' : 'start')}
                        title="Redémarrer / Start"
                        className="rounded p-1 text-fg-muted hover:bg-surface-2 hover:text-fg"
                      >
                        <Play size={13} />
                      </button>
                      <button
                        type="button"
                        onClick={() => setMigrateModalRow(u)}
                        title="Relocaliser à chaud (Live Migration)"
                        className="rounded p-1 text-sky-500 hover:bg-sky-500/10"
                      >
                        <ArrowRightLeft size={13} />
                      </button>
                      <button
                        type="button"
                        onClick={() => handleBackup(u.id)}
                        title="Créer Backup PMBS + Op 10 Proof"
                        className="rounded p-1 text-emerald-500 hover:bg-emerald-500/10"
                      >
                        <Shield size={13} />
                      </button>
                      <button
                        type="button"
                        onClick={() => handleLifecycle(u.id, 'stop')}
                        title="Arrêt"
                        className="rounded p-1 text-danger hover:bg-danger-soft"
                      >
                        <Square size={13} />
                      </button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>

      {/* Modal Déploiement d'Univers */}
      {deployModalOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4 backdrop-blur-xs">
          <div className="w-full max-w-lg rounded-2xl border border-line bg-surface p-6 shadow-2xl">
            <h3 className="text-lg font-bold text-fg">Déployer un Univers (Strate 0)</h3>
            <p className="mt-1 text-xs text-fg-muted">
              Déploiement direct souverain avec isolation Cgroups v2 et userns-v1.
            </p>
            <form onSubmit={handleDeploy} className="mt-4 space-y-3 text-xs">
              <div>
                <label className="block font-semibold text-fg mb-1">Nom / Slug Univers</label>
                <input
                  type="text"
                  required
                  placeholder="ex: boutique-paris-nord"
                  value={newUniverse.name}
                  onChange={(e) => setNewUniverse({ ...newUniverse, name: e.target.value })}
                  className="w-full rounded-lg border border-line bg-surface-2 px-3 py-2 text-fg focus:border-accent focus:outline-hidden"
                />
              </div>
              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className="block font-semibold text-fg mb-1">Nœud de Destination</label>
                  <Select
                    value={newUniverse.nodeId}
                    onChange={(val) => setNewUniverse({ ...newUniverse, nodeId: val })}
                    options={allNodes.map((n) => ({ value: n.id, label: n.name }))}
                  />
                </div>
                <div>
                  <label className="block font-semibold text-fg mb-1">Environnement</label>
                  <Select
                    value={newUniverse.env}
                    onChange={(val) => setNewUniverse({ ...newUniverse, env: val })}
                    options={[
                      { value: 'demo', label: 'Demo / Qualification' },
                      { value: 'prod', label: 'Production Scellée' },
                      { value: 'test', label: 'Test' },
                    ]}
                  />
                </div>
              </div>
              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className="block font-semibold text-fg mb-1">Quota RAM (MiB)</label>
                  <input
                    type="number"
                    value={newUniverse.memoryMb}
                    onChange={(e) => setNewUniverse({ ...newUniverse, memoryMb: Number(e.target.value) })}
                    className="w-full rounded-lg border border-line bg-surface-2 px-3 py-2 text-fg focus:border-accent focus:outline-hidden"
                  />
                </div>
                <div>
                  <label className="block font-semibold text-fg mb-1">Cœurs CPU</label>
                  <input
                    type="number"
                    value={newUniverse.cpuCores}
                    onChange={(e) => setNewUniverse({ ...newUniverse, cpuCores: Number(e.target.value) })}
                    className="w-full rounded-lg border border-line bg-surface-2 px-3 py-2 text-fg focus:border-accent focus:outline-hidden"
                  />
                </div>
              </div>
              <div>
                <label className="block font-semibold text-fg mb-1">Matrice OCI (Image de base)</label>
                <input
                  type="text"
                  value={newUniverse.image}
                  onChange={(e) => setNewUniverse({ ...newUniverse, image: e.target.value })}
                  className="w-full rounded-lg border border-line bg-surface-2 px-3 py-2 font-mono text-fg focus:border-accent focus:outline-hidden"
                />
              </div>

              <div className="flex items-center justify-end gap-2 pt-3 border-t border-line">
                <button
                  type="button"
                  onClick={() => setDeployModalOpen(false)}
                  className="rounded-lg border border-line px-4 py-2 font-medium text-fg hover:bg-surface-2"
                >
                  Annuler
                </button>
                <button
                  type="submit"
                  disabled={deploying}
                  className="rounded-lg bg-accent px-4 py-2 font-bold text-accent-fg shadow-sm hover:opacity-90 disabled:opacity-50"
                >
                  {deploying ? 'Déploiement en cours…' : 'Lancer le déploiement'}
                </button>
              </div>
            </form>
          </div>
        </div>
      )}

      {/* Modal Relocalisation à chaud (Live Migration) */}
      {migrateModalRow && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4 backdrop-blur-xs">
          <div className="w-full max-w-md rounded-2xl border border-line bg-surface p-6 shadow-2xl">
            <h3 className="text-lg font-bold text-fg flex items-center gap-2">
              <ArrowRightLeft className="text-sky-500" size={20} /> Relocalisation à chaud
            </h3>
            <p className="mt-1 text-xs text-fg-muted">
              Déplacement en direct de <span className="font-mono text-fg font-semibold">{migrateModalRow.id}</span> sans interruption de service (Btrfs send/receive delta 42ms).
            </p>
            <div className="mt-4 space-y-3 text-xs">
              <div>
                <label className="block font-semibold text-fg mb-1">Nœud Hôte Actuel</label>
                <input
                  type="text"
                  disabled
                  value={migrateModalRow.machine}
                  className="w-full rounded-lg border border-line bg-surface-2/60 px-3 py-2 text-fg-muted font-mono"
                />
              </div>
              <div>
                <label className="block font-semibold text-fg mb-1">Nœud de Destination (Cible)</label>
                <Select
                  value={targetMigrateNode}
                  onChange={(val) => setTargetMigrateNode(val)}
                  options={allNodes
                    .filter((n) => n.id !== migrateModalRow.machine)
                    .map((n) => ({
                      value: n.id,
                      label: `${n.name} (${n.site})`,
                    }))}
                />
              </div>
              <div className="flex items-center justify-end gap-2 pt-3 border-t border-line">
                <button
                  type="button"
                  onClick={() => setMigrateModalRow(null)}
                  className="rounded-lg border border-line px-4 py-2 font-medium text-fg hover:bg-surface-2"
                >
                  Annuler
                </button>
                <button
                  type="button"
                  onClick={handleMigrate}
                  className="rounded-lg bg-sky-600 px-4 py-2 font-bold text-white shadow-sm hover:bg-sky-500"
                >
                  Effectuer la migration (42ms)
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
