import { useEffect, useState } from 'react'
import {
  Archive,
  ArrowRightLeft,
  CheckCircle2,
  Clock,
  Download,
  FileText,
  GitBranch,
  HardDrive,
  Plus,
  RefreshCw,
  Server,
  Shield,
  Zap,
} from 'lucide-react'
import { api } from '../api/client'
import { showToast } from '../components/ui/ToastHost'

export default function PmbsPage() {
  const [data, setData] = useState(null)
  const [loading, setLoading] = useState(true)
  const [timeTravelModalBackup, setTimeTravelModalBackup] = useState(null)
  const [branchName, setBranchName] = useState('')
  const [restoreFileModalBackup, setRestoreFileModalBackup] = useState(null)
  const [restoreFilePath, setRestoreFilePath] = useState('/etc/shaper/status.json')
  const [restoreResult, setRestoreResult] = useState(null)
  const [addServerModalOpen, setAddServerModalOpen] = useState(false)
  const [newServer, setNewServer] = useState({
    id: 'pmbs-external-01',
    name: 'PMBS Baremetal Sauvegarde Dédié',
    ip: '10.10.10.252',
    site: 'external',
    hasPodmeshRuntime: false,
    totalGb: 500,
  })

  async function loadData() {
    try {
      const res = await api.podmesh.backups()
      setData(res)
    } catch (err) {
      showToast({ title: 'Erreur PMBS', description: err.message, variant: 'danger' })
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    loadData()
  }, [])

  async function handleTimeTravelFork(e) {
    e.preventDefault()
    if (!timeTravelModalBackup) return
    try {
      const res = await api.podmesh.timeTravelFork(timeTravelModalBackup.id, branchName)
      showToast({
        title: 'Branche CoW Créée',
        description: `Nouvel univers ${res.forkedUniverse?.id} cloné en 310ms!`,
        variant: 'success',
      })
      setTimeTravelModalBackup(null)
      setBranchName('')
      loadData()
    } catch (err) {
      showToast({ title: 'Échec Time-Travel', description: err.message, variant: 'danger' })
    }
  }

  async function handleRestoreFile(e) {
    e.preventDefault()
    if (!restoreFileModalBackup) return
    try {
      const res = await api.podmesh.restoreFile(restoreFileModalBackup.id, restoreFilePath)
      setRestoreResult(res)
      showToast({ title: 'Fichier extrait', description: `${res.filePath} extrait avec succès (${res.restoredBytes} octets)`, variant: 'success' })
    } catch (err) {
      showToast({ title: 'Échec extraction', description: err.message, variant: 'danger' })
    }
  }

  async function handleAddServer(e) {
    e.preventDefault()
    try {
      await api.podmesh.addPmbsServer(newServer)
      showToast({ title: 'Serveur PMBS ajouté', description: `Serveur ${newServer.name} configuré (${newServer.hasPodmeshRuntime ? 'avec' : 'SANS'} PodMesh)`, variant: 'success' })
      setAddServerModalOpen(false)
      loadData()
    } catch (err) {
      showToast({ title: 'Échec ajout serveur', description: err.message, variant: 'danger' })
    }
  }

  if (loading && !data) {
    return (
      <div className="flex h-64 items-center justify-center">
        <RefreshCw className="animate-spin text-accent" size={32} />
      </div>
    )
  }

  const servers = data?.pmbsServers || []
  const backups = data?.backups || []

  return (
    <div className="space-y-6">
      {/* Top Banner */}
      <div className="flex flex-col justify-between gap-4 rounded-2xl border border-line bg-surface p-5 shadow-sm sm:flex-row sm:items-center">
        <div>
          <h1 className="text-xl font-black text-fg flex items-center gap-2">
            <Shield className="text-accent" size={24} /> PodMesh Backup Server (PMBS)
          </h1>
          <p className="mt-1 text-xs text-fg-muted">
            Architecture Proxmox Backup Server : Datastores dédupliqués, synchronisation cross-site continue, et Preuve Sandbox Boot-Restore Op 10.
          </p>
        </div>

        <button
          type="button"
          onClick={() => setAddServerModalOpen(true)}
          className="flex items-center gap-1.5 rounded-lg bg-accent px-3 py-1.5 text-xs font-bold text-accent-fg shadow-sm hover:opacity-90"
        >
          <Plus size={15} /> Ajouter Serveur PMBS
        </button>
      </div>

      {/* PMBS Dedicated Servers Grid */}
      <section className="space-y-3">
        <h2 className="text-base font-bold text-fg flex items-center gap-2">
          <Server size={18} className="text-accent" /> Serveurs PMBS Actifs & Datastores Dédupliqués ({servers.length})
        </h2>
        <div className="grid gap-4 md:grid-cols-2">
          {servers.map((srv) => (
            <div key={srv.id} className="rounded-xl border border-line bg-surface p-4 shadow-sm">
              <div className="flex items-start justify-between">
                <div>
                  <h3 className="text-sm font-bold text-fg">{srv.name}</h3>
                  <p className="font-mono text-xs text-accent">{srv.datastore} • {srv.site}</p>
                </div>
                <span className="inline-flex items-center gap-1 rounded-full bg-emerald-500/10 px-2 py-0.5 text-xs font-bold text-emerald-500">
                  <CheckCircle2 size={13} /> {srv.status}
                </span>
              </div>

              <div className="mt-3 grid grid-cols-3 gap-2 border-y border-line/60 py-2.5 text-center text-xs">
                <div>
                  <p className="text-[10px] uppercase font-bold text-fg-faint">Déduplication</p>
                  <p className="mt-1 font-mono font-bold text-emerald-400">{srv.dedupFactor}x</p>
                </div>
                <div>
                  <p className="text-[10px] uppercase font-bold text-fg-faint">Espace Utilisé</p>
                  <p className="mt-1 font-mono font-bold text-fg">{srv.usedGb} / {srv.totalGb} GiB</p>
                </div>
                <div>
                  <p className="text-[10px] uppercase font-bold text-fg-faint">Preuve Op 10</p>
                  <p className="mt-1 font-mono font-bold text-emerald-500">{srv.op10VerifierStatus}</p>
                </div>
              </div>

              <div className="mt-3 flex items-center justify-between text-xs text-fg-muted">
                <span>Sync Cible : <span className="font-mono text-fg">{srv.remoteSyncTarget}</span></span>
                <span className={srv.requiresPodmeshOnHost ? 'text-emerald-500' : 'text-amber-400 font-medium'}>
                  {srv.deploymentMode}
                </span>
              </div>
            </div>
          ))}
        </div>
      </section>

      {/* Op 10 Boot-Restore Verified Snapshots List */}
      <section className="space-y-3">
        <h2 className="text-base font-bold text-fg flex items-center gap-2">
          <Archive size={18} className="text-indigo-400" /> Snapshots Immuables & Preuves Boot-Restore Op 10 ({backups.length})
        </h2>

        <div className="overflow-x-auto rounded-xl border border-line bg-surface shadow-sm">
          <table className="w-full text-left text-xs">
            <thead className="border-b border-line bg-surface-2/60 text-fg-muted uppercase tracking-wider text-[10px]">
              <tr>
                <th className="px-4 py-3">Snapshot / ID</th>
                <th className="px-3 py-3">Univers Source</th>
                <th className="px-3 py-3">Datastore & Réplique</th>
                <th className="px-3 py-3">Taille (Delta)</th>
                <th className="px-3 py-3">Preuve Op 10 Boot-Restore</th>
                <th className="px-3 py-3 text-right">Actions Restauration</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-line/60">
              {backups.map((b) => (
                <tr key={b.id} className="hover:bg-surface-2/40 transition-colors">
                  <td className="px-4 py-3 font-mono">
                    <div className="font-bold text-fg">{b.id}</div>
                    <div className="text-[10px] text-fg-muted truncate max-w-[180px]">{b.digest}</div>
                  </td>
                  <td className="px-3 py-3">
                    <span className="font-medium text-fg">{b.universeId}</span>
                    <div className="text-[10px] text-fg-muted">{b.site}</div>
                  </td>
                  <td className="px-3 py-3 font-mono text-[11px]">
                    <div className="text-fg">{b.pmbsId}</div>
                    <div className="text-sky-400 flex items-center gap-1">
                      <ArrowRightLeft size={11} /> {b.replicatedTo}
                    </div>
                  </td>
                  <td className="px-3 py-3 font-mono">
                    <div>{b.sizeMb} MiB</div>
                    <div className="text-[10px] text-emerald-400">+{b.deltaMb} MiB delta</div>
                  </td>
                  <td className="px-3 py-3">
                    {b.op10BootRestore?.verified ? (
                      <div className="rounded-lg bg-emerald-500/10 p-1.5 text-[11px] text-emerald-400 font-semibold inline-block">
                        ✓ PASS en {b.op10BootRestore.bootDurationMs}ms ({b.op10BootRestore.targetNode})
                      </div>
                    ) : (
                      <span className="text-amber-500">En cours de vérification</span>
                    )}
                  </td>
                  <td className="px-3 py-3 text-right">
                    <div className="flex items-center justify-end gap-1.5">
                      <button
                        type="button"
                        onClick={() => {
                          setRestoreFileModalBackup(b)
                          setRestoreResult(null)
                        }}
                        className="flex items-center gap-1 rounded bg-surface-2 px-2 py-1 text-[11px] font-semibold text-fg hover:bg-surface border border-line"
                        title="Extraction d'un fichier unitaire"
                      >
                        <FileText size={12} /> Restaurer Fichier
                      </button>
                      <button
                        type="button"
                        onClick={() => setTimeTravelModalBackup(b)}
                        className="flex items-center gap-1 rounded bg-indigo-600 px-2.5 py-1 text-[11px] font-bold text-white hover:bg-indigo-500 shadow-sm"
                        title="Créer une branche parallèle temporelle en CoW"
                      >
                        <GitBranch size={12} /> Time-Machine Fork
                      </button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>

      {/* Modal Time-Machine CoW Parallel Branch Fork */}
      {timeTravelModalBackup && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4 backdrop-blur-xs">
          <div className="w-full max-w-md rounded-2xl border border-line bg-surface p-6 shadow-2xl">
            <h3 className="text-lg font-bold text-fg flex items-center gap-2">
              <GitBranch className="text-indigo-400" size={20} /> Time-Machine CoW Fork
            </h3>
            <p className="mt-1 text-xs text-fg-muted">
              Ouvrir instantanément une branche temporelle clone en CoW (Copy-on-Write) à partir de <span className="font-mono text-fg">{timeTravelModalBackup.id}</span>.
            </p>
            <form onSubmit={handleTimeTravelFork} className="mt-4 space-y-3 text-xs">
              <div>
                <label className="block font-semibold text-fg mb-1">Nom de la Branche Sandbox</label>
                <input
                  type="text"
                  required
                  placeholder="ex: audit-financier-branche-01"
                  value={branchName}
                  onChange={(e) => setBranchName(e.target.value)}
                  className="w-full rounded-lg border border-line bg-surface-2 px-3 py-2 text-fg focus:border-accent focus:outline-hidden"
                />
              </div>
              <div className="rounded-lg bg-surface-2 p-3 font-mono text-[11px] text-fg-muted space-y-1">
                <div>Univers Source : {timeTravelModalBackup.universeId}</div>
                <div>Horodatage Snapshot : {timeTravelModalBackup.createdAt}</div>
                <div>Vitesse de fork estimée : ~310ms (zéro copie disque)</div>
              </div>
              <div className="flex items-center justify-end gap-2 pt-3 border-t border-line">
                <button
                  type="button"
                  onClick={() => setTimeTravelModalBackup(null)}
                  className="rounded-lg border border-line px-4 py-2 font-medium text-fg hover:bg-surface-2"
                >
                  Annuler
                </button>
                <button
                  type="submit"
                  className="rounded-lg bg-indigo-600 px-4 py-2 font-bold text-white shadow-sm hover:bg-indigo-500"
                >
                  Ouvrir la Branche CoW
                </button>
              </div>
            </form>
          </div>
        </div>
      )}

      {/* Modal Single-File Restore */}
      {restoreFileModalBackup && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4 backdrop-blur-xs">
          <div className="w-full max-w-lg rounded-2xl border border-line bg-surface p-6 shadow-2xl">
            <h3 className="text-lg font-bold text-fg flex items-center gap-2">
              <FileText className="text-accent" size={20} /> Restauration Unitaire de Fichier (File-Level Restore)
            </h3>
            <p className="mt-1 text-xs text-fg-muted">
              Extraire un fichier précis de l'archive PMBS dédupliquée sans restaurer l'intégralité de la VM.
            </p>
            <form onSubmit={handleRestoreFile} className="mt-4 space-y-3 text-xs">
              <div>
                <label className="block font-semibold text-fg mb-1">Chemin Absolu du Fichier</label>
                <input
                  type="text"
                  required
                  value={restoreFilePath}
                  onChange={(e) => setRestoreFilePath(e.target.value)}
                  className="w-full rounded-lg border border-line bg-surface-2 px-3 py-2 font-mono text-fg focus:border-accent focus:outline-hidden"
                />
              </div>

              <div>
                <p className="font-semibold text-fg mb-1">Fichiers fréquents dans cette archive :</p>
                <div className="flex flex-wrap gap-1.5">
                  {(restoreFileModalBackup.filesPreview || []).map((f) => (
                    <button
                      key={f}
                      type="button"
                      onClick={() => setRestoreFilePath(f)}
                      className="rounded bg-surface-2 px-2 py-1 font-mono text-[10px] text-accent hover:bg-surface border border-line"
                    >
                      {f}
                    </button>
                  ))}
                </div>
              </div>

              {restoreResult && (
                <div className="rounded-lg border border-emerald-500/30 bg-emerald-500/10 p-3">
                  <div className="font-bold text-emerald-400">✓ Fichier extrait avec succès !</div>
                  <pre className="mt-2 overflow-x-auto rounded bg-surface p-2 font-mono text-[10px] text-fg-muted">
                    {restoreResult.contentPreview}
                  </pre>
                </div>
              )}

              <div className="flex items-center justify-end gap-2 pt-3 border-t border-line">
                <button
                  type="button"
                  onClick={() => setRestoreFileModalBackup(null)}
                  className="rounded-lg border border-line px-4 py-2 font-medium text-fg hover:bg-surface-2"
                >
                  Fermer
                </button>
                <button
                  type="submit"
                  className="rounded-lg bg-accent px-4 py-2 font-bold text-accent-fg shadow-sm hover:opacity-90"
                >
                  Extraire le Fichier
                </button>
              </div>
            </form>
          </div>
        </div>
      )}

      {/* Modal Ajout Serveur PMBS */}
      {addServerModalOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4 backdrop-blur-xs">
          <div className="w-full max-w-lg rounded-2xl border border-line bg-surface p-6 shadow-2xl">
            <h3 className="text-lg font-bold text-fg">Ajouter un Serveur PMBS</h3>
            <p className="mt-1 text-xs text-fg-muted">
              Note essentielle : Le PodMesh Backup Server peut être installé sur n'importe quel serveur Linux <strong>avec OU sans PodMesh</strong>.
            </p>
            <form onSubmit={handleAddServer} className="mt-4 space-y-3 text-xs">
              <div>
                <label className="block font-semibold text-fg mb-1">Identifiant Serveur</label>
                <input
                  type="text"
                  required
                  value={newServer.id}
                  onChange={(e) => setNewServer({ ...newServer, id: e.target.value })}
                  className="w-full rounded-lg border border-line bg-surface-2 px-3 py-2 text-fg focus:border-accent focus:outline-hidden"
                />
              </div>
              <div>
                <label className="block font-semibold text-fg mb-1">Nom descriptif</label>
                <input
                  type="text"
                  required
                  value={newServer.name}
                  onChange={(e) => setNewServer({ ...newServer, name: e.target.value })}
                  className="w-full rounded-lg border border-line bg-surface-2 px-3 py-2 text-fg focus:border-accent focus:outline-hidden"
                />
              </div>
              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className="block font-semibold text-fg mb-1">Adresse IP</label>
                  <input
                    type="text"
                    required
                    value={newServer.ip}
                    onChange={(e) => setNewServer({ ...newServer, ip: e.target.value })}
                    className="w-full rounded-lg border border-line bg-surface-2 px-3 py-2 text-fg focus:border-accent focus:outline-hidden"
                  />
                </div>
                <div>
                  <label className="block font-semibold text-fg mb-1">Capacité Datastore (GiB)</label>
                  <input
                    type="number"
                    value={newServer.totalGb}
                    onChange={(e) => setNewServer({ ...newServer, totalGb: Number(e.target.value) })}
                    className="w-full rounded-lg border border-line bg-surface-2 px-3 py-2 text-fg focus:border-accent focus:outline-hidden"
                  />
                </div>
              </div>
              <div>
                <label className="flex items-center gap-2 cursor-pointer mt-2">
                  <input
                    type="checkbox"
                    checked={newServer.hasPodmeshRuntime}
                    onChange={(e) => setNewServer({ ...newServer, hasPodmeshRuntime: e.target.checked })}
                    className="h-4 w-4 rounded border-line text-accent"
                  />
                  <span className="font-semibold text-fg">
                    PodMesh Runtime est installé sur ce serveur (si décoché : serveur autonome sans PodMesh)
                  </span>
                </label>
              </div>

              <div className="flex items-center justify-end gap-2 pt-3 border-t border-line">
                <button
                  type="button"
                  onClick={() => setAddServerModalOpen(false)}
                  className="rounded-lg border border-line px-4 py-2 font-medium text-fg hover:bg-surface-2"
                >
                  Annuler
                </button>
                <button
                  type="submit"
                  className="rounded-lg bg-accent px-4 py-2 font-bold text-accent-fg shadow-sm hover:opacity-90"
                >
                  Enregistrer Serveur PMBS
                </button>
              </div>
            </form>
          </div>
        </div>
      )}
    </div>
  )
}
