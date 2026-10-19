import { useEffect, useState } from 'react'
import {
  Boxes,
  CheckCircle2,
  Database,
  FolderTree,
  HardDrive,
  Layers,
  Plus,
  RefreshCw,
  Server,
  Sparkles,
} from 'lucide-react'
import { api } from '../api/client'
import { showToast } from '../components/ui/ToastHost'
import Select from '../components/ui/Select'

export default function StoragePage() {
  const [data, setData] = useState({ registries: [], storages: [] })
  const [loading, setLoading] = useState(true)
  const [addStorageOpen, setAddStorageOpen] = useState(false)
  const [newPool, setNewPool] = useState({
    id: 'stg-pool-01',
    name: 'Stockage Partagé MeshFS',
    site: 'gbs-p4',
    type: 'meshfs-merkle',
    path: '/meshfs/universes',
    totalGb: 500,
    hasPodmeshRuntime: false,
  })

  async function loadData() {
    try {
      const [regRes, stgRes] = await Promise.all([api.podmesh.registries(), api.podmesh.storage()])
      setData({ registries: regRes.registries || [], storages: stgRes.storages || [] })
    } catch (err) {
      showToast({ title: 'Erreur Stockage', description: err.message, variant: 'danger' })
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    loadData()
  }, [])

  async function handleAddStorage(e) {
    e.preventDefault()
    try {
      await api.podmesh.addStorage(newPool)
      showToast({ title: 'Pool de stockage ajouté', description: `Pool ${newPool.name} créé (${newPool.hasPodmeshRuntime ? 'avec' : 'SANS'} PodMesh requis)`, variant: 'success' })
      setAddStorageOpen(false)
      loadData()
    } catch (err) {
      showToast({ title: 'Échec ajout pool', description: err.message, variant: 'danger' })
    }
  }

  if (loading && !data.registries.length) {
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
            <HardDrive className="text-accent" size={24} /> Stockages & Registres OCI
          </h1>
          <p className="mt-1 text-xs text-fg-muted">
            Pools Btrfs CoW, Registres Conteneurs Dédiés (CT 9211 / CT 9111) et Aperçu MeshFS (Système de fichiers distribué élastique de 1 à N nœuds).
          </p>
        </div>

        <button
          type="button"
          onClick={() => setAddStorageOpen(true)}
          className="flex items-center gap-1.5 rounded-lg bg-accent px-3 py-1.5 text-xs font-bold text-accent-fg shadow-sm hover:opacity-90"
        >
          <Plus size={15} /> Ajouter Pool de Stockage
        </button>
      </div>

      {/* MeshFS Highlight Card */}
      <div className="rounded-2xl border border-accent/40 bg-accent/5 p-5 shadow-sm space-y-2">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <Sparkles className="text-accent" size={20} />
            <h3 className="text-base font-bold text-fg">MeshFS — Système de Fichiers Distribué Élastique (Phase F)</h3>
          </div>
          <span className="rounded bg-accent/20 px-2 py-0.5 text-xs font-bold text-accent">
            Prévu en Phase Finale (Phase F)
          </span>
        </div>
        <p className="text-xs text-fg-muted leading-relaxed">
          Inspiré de CephFS mais infiniment plus simple à déployer : fonctionne de 1 nœud seul jusqu'à N nœuds élastiques, sur des serveurs <strong>avec OU sans PodMesh installé</strong>, avec déduplication et sauvegardes incrémentales structurelles (Merkle trees).
        </p>
      </div>

      {/* Storage Pools List */}
      <section className="space-y-3">
        <h2 className="text-base font-bold text-fg flex items-center gap-2">
          <FolderTree size={18} className="text-accent" /> Pools de Stockage Actifs ({data.storages.length})
        </h2>
        <div className="grid gap-4 md:grid-cols-3">
          {data.storages.map((stg) => (
            <div key={stg.id} className="rounded-xl border border-line bg-surface p-4 shadow-sm space-y-3">
              <div className="flex items-start justify-between">
                <div>
                  <h3 className="text-sm font-bold text-fg">{stg.name}</h3>
                  <p className="font-mono text-xs text-accent">{stg.type}</p>
                </div>
                <span className="h-2 w-2 rounded-full bg-emerald-500" />
              </div>
              <div className="text-xs space-y-1 text-fg-muted">
                <div>Point de montage : <span className="font-mono text-fg">{stg.path}</span></div>
                <div>Capacité : <span className="font-mono text-fg">{stg.usedGb} / {stg.totalGb} GiB</span></div>
                <div>Déploiement : <span className={stg.requiresPodmeshOnHost ? 'text-emerald-500' : 'text-amber-400 font-medium'}>{stg.deploymentMode}</span></div>
              </div>
              <div className="flex flex-wrap gap-1 pt-1 border-t border-line/60">
                {(stg.features || []).map((feat) => (
                  <span key={feat} className="rounded bg-surface-2 px-1.5 py-0.5 text-[10px] font-mono text-fg-muted">
                    {feat}
                  </span>
                ))}
              </div>
            </div>
          ))}
        </div>
      </section>

      {/* OCI Registries */}
      <section className="space-y-3">
        <h2 className="text-base font-bold text-fg flex items-center gap-2">
          <Database size={18} className="text-sky-400" /> Registres OCI Dédiés ({data.registries.length})
        </h2>
        <div className="grid gap-4 md:grid-cols-2">
          {data.registries.map((reg) => (
            <div key={reg.id} className="rounded-xl border border-line bg-surface p-4 shadow-sm space-y-3">
              <div className="flex items-center justify-between">
                <div>
                  <h3 className="text-sm font-bold text-fg">{reg.name}</h3>
                  <p className="font-mono text-xs text-fg-muted">{reg.endpoint} • CT {reg.ctid}</p>
                </div>
                <span className="rounded bg-emerald-500/10 px-2 py-0.5 text-xs font-bold text-emerald-500">
                  {reg.status}
                </span>
              </div>
              <div className="text-xs text-fg-muted">
                Backend Stockage : <span className="font-mono text-fg">{reg.storageBackend}</span>
              </div>
              <div className="space-y-2">
                <p className="text-[11px] font-semibold text-fg-muted">Images Scellées & Vérifiées :</p>
                <div className="space-y-1.5">
                  {(reg.images || []).map((img) => (
                    <div key={`${img.repository}:${img.tag}`} className="rounded bg-surface-2 p-2 text-xs border border-line flex items-center justify-between">
                      <div>
                        <div className="font-mono font-bold text-fg">{img.repository}:{img.tag}</div>
                        <div className="font-mono text-[10px] text-fg-muted truncate max-w-[280px]">{img.digest}</div>
                      </div>
                      <span className="font-mono text-[11px] text-fg-muted">{img.sizeMb} MiB</span>
                    </div>
                  ))}
                </div>
              </div>
            </div>
          ))}
        </div>
      </section>

      {/* Modal Ajout Stockage */}
      {addStorageOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4 backdrop-blur-xs">
          <div className="w-full max-w-md rounded-2xl border border-line bg-surface p-6 shadow-2xl">
            <h3 className="text-lg font-bold text-fg">Ajouter un Pool de Stockage</h3>
            <p className="mt-1 text-xs text-fg-muted">
              Prend en charge les pools Btrfs locaux ou MeshFS distribués.
            </p>
            <form onSubmit={handleAddStorage} className="mt-4 space-y-3 text-xs">
              <div>
                <label className="block font-semibold text-fg mb-1">Identifiant Pool</label>
                <input
                  type="text"
                  required
                  value={newPool.id}
                  onChange={(e) => setNewPool({ ...newPool, id: e.target.value })}
                  className="w-full rounded-lg border border-line bg-surface-2 px-3 py-2 text-fg focus:border-accent focus:outline-hidden"
                />
              </div>
              <div>
                <label className="block font-semibold text-fg mb-1">Nom du Pool</label>
                <input
                  type="text"
                  required
                  value={newPool.name}
                  onChange={(e) => setNewPool({ ...newPool, name: e.target.value })}
                  className="w-full rounded-lg border border-line bg-surface-2 px-3 py-2 text-fg focus:border-accent focus:outline-hidden"
                />
              </div>
              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className="block font-semibold text-fg mb-1">Type de Stockage</label>
                  <Select
                    value={newPool.type}
                    onChange={(val) => setNewPool({ ...newPool, type: val })}
                    options={[
                      { value: 'meshfs-merkle', label: 'MeshFS (Merkle Distribué)' },
                      { value: 'btrfs-cow', label: 'Btrfs CoW Local' },
                    ]}
                  />
                </div>
                <div>
                  <label className="block font-semibold text-fg mb-1">Capacité (GiB)</label>
                  <input
                    type="number"
                    value={newPool.totalGb}
                    onChange={(e) => setNewPool({ ...newPool, totalGb: Number(e.target.value) })}
                    className="w-full rounded-lg border border-line bg-surface-2 px-3 py-2 text-fg focus:border-accent focus:outline-hidden"
                  />
                </div>
              </div>
              <div>
                <label className="block font-semibold text-fg mb-1">Chemin de montage</label>
                <input
                  type="text"
                  value={newPool.path}
                  onChange={(e) => setNewPool({ ...newPool, path: e.target.value })}
                  className="w-full rounded-lg border border-line bg-surface-2 px-3 py-2 font-mono text-fg focus:border-accent focus:outline-hidden"
                />
              </div>

              <div className="flex items-center justify-end gap-2 pt-3 border-t border-line">
                <button
                  type="button"
                  onClick={() => setAddStorageOpen(false)}
                  className="rounded-lg border border-line px-4 py-2 font-medium text-fg hover:bg-surface-2"
                >
                  Annuler
                </button>
                <button
                  type="submit"
                  className="rounded-lg bg-accent px-4 py-2 font-bold text-accent-fg shadow-sm hover:opacity-90"
                >
                  Créer le Pool
                </button>
              </div>
            </form>
          </div>
        </div>
      )}
    </div>
  )
}
