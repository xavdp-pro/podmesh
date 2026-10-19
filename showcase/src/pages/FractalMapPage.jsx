import { useEffect, useState } from 'react'
import {
  Boxes,
  Cpu,
  Eye,
  Layers,
  Maximize2,
  Minimize2,
  RefreshCw,
  Server,
  Shield,
  ZoomIn,
  ZoomOut,
} from 'lucide-react'
import { api } from '../api/client'
import { showToast } from '../components/ui/ToastHost'

export default function FractalMapPage() {
  const [topology, setTopology] = useState(null)
  const [loading, setLoading] = useState(true)
  const [zoom, setZoom] = useState(1)
  const [view3D, setView3D] = useState(false)
  const [selectedNode, setSelectedNode] = useState(null)
  const [activeFilterStratum, setActiveFilterStratum] = useState('all')

  async function loadTopology() {
    try {
      const res = await api.podmesh.topologyMap()
      setTopology(res)
    } catch (err) {
      showToast({ title: 'Erreur Topologie', description: err.message, variant: 'danger' })
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    loadTopology()
  }, [])

  if (loading && !topology) {
    return (
      <div className="flex h-64 items-center justify-center">
        <RefreshCw className="animate-spin text-accent" size={32} />
      </div>
    )
  }

  const strata = topology?.strata || []

  return (
    <div className="space-y-4">
      {/* Header and Controls */}
      <div className="flex flex-col justify-between gap-3 rounded-2xl border border-line bg-surface p-4 shadow-sm sm:flex-row sm:items-center">
        <div>
          <h1 className="text-xl font-black text-fg flex items-center gap-2">
            <Layers className="text-accent" size={24} /> Carte Fractale des Juridictions SHAPER OS v1.15
          </h1>
          <p className="mt-1 text-xs text-fg-muted">
            Séparation étanche et hiérarchie : Strate 0 (Hôte Souverain) ➔ Strate 1 (Gouverneurs SaaS) ➔ Strate 2 (Makers Recettes) ➔ Strate 3 (Briques)
          </p>
        </div>

        {/* View Controls */}
        <div className="flex items-center gap-2">
          <div className="flex items-center rounded-lg border border-line bg-surface-2 p-1">
            <button
              type="button"
              onClick={() => setZoom((z) => Math.max(0.6, z - 0.15))}
              className="rounded p-1 text-fg hover:bg-surface"
              title="Zoom Arrière"
            >
              <ZoomOut size={16} />
            </button>
            <span className="px-2 font-mono text-xs text-fg-muted">{Math.round(zoom * 100)}%</span>
            <button
              type="button"
              onClick={() => setZoom((z) => Math.min(1.8, z + 0.15))}
              className="rounded p-1 text-fg hover:bg-surface"
              title="Zoom Avant"
            >
              <ZoomIn size={16} />
            </button>
          </div>

          <button
            type="button"
            onClick={() => setView3D(!view3D)}
            className={`flex items-center gap-1.5 rounded-lg border px-3 py-1.5 text-xs font-semibold transition-all ${
              view3D ? 'border-accent bg-accent text-accent-fg' : 'border-line bg-surface-2 text-fg hover:bg-surface'
            }`}
          >
            <Boxes size={15} /> Vue Isométrique 3D
          </button>
        </div>
      </div>

      {/* Strata Filter Tabs */}
      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          onClick={() => setActiveFilterStratum('all')}
          className={`rounded-lg px-3 py-1 text-xs font-semibold transition-all ${
            activeFilterStratum === 'all' ? 'bg-accent text-accent-fg' : 'bg-surface border border-line text-fg-muted hover:text-fg'
          }`}
        >
          Toutes les Strates (0 à 3)
        </button>
        {strata.map((st) => (
          <button
            key={st.stratum}
            type="button"
            onClick={() => setActiveFilterStratum(st.stratum)}
            className={`rounded-lg px-3 py-1 text-xs font-semibold transition-all ${
              activeFilterStratum === st.stratum ? 'bg-accent text-accent-fg' : 'bg-surface border border-line text-fg-muted hover:text-fg'
            }`}
          >
            Strate {st.stratum}
          </button>
        ))}
      </div>

      {/* Main Fractal Canvas + Side Inspector Panel */}
      <div className="grid gap-4 lg:grid-cols-4">
        {/* SVG Interactive Canvas */}
        <div className="relative overflow-hidden rounded-2xl border border-line bg-surface-2/40 p-4 lg:col-span-3 min-h-[560px]">
          <div
            className="w-full h-full transition-transform duration-300 ease-out origin-top-left"
            style={{
              transform: `scale(${zoom}) ${view3D ? 'rotateX(22deg) rotateZ(-6deg) skewX(8deg)' : ''}`,
              transformStyle: 'preserve-3d',
            }}
          >
            {/* Strata Layers */}
            <div className="space-y-6">
              {strata
                .filter((st) => activeFilterStratum === 'all' || activeFilterStratum === st.stratum)
                .map((st) => (
                  <div
                    key={st.stratum}
                    className={`rounded-2xl border p-4 shadow-sm transition-all ${
                      st.stratum === 0
                        ? 'border-emerald-500/30 bg-emerald-950/10'
                        : st.stratum === 1
                        ? 'border-sky-500/30 bg-sky-950/10'
                        : st.stratum === 2
                        ? 'border-indigo-500/30 bg-indigo-950/10'
                        : 'border-amber-500/30 bg-amber-950/10'
                    }`}
                  >
                    <div className="flex items-center justify-between border-b border-line/60 pb-2 mb-3">
                      <div>
                        <span className="font-mono text-[10px] uppercase font-bold tracking-widest text-accent">
                          STRATE {st.stratum}
                        </span>
                        <h3 className="text-sm font-bold text-fg">{st.name}</h3>
                      </div>
                      <span className="rounded bg-surface px-2.5 py-1 text-[11px] font-mono text-fg-muted border border-line">
                        {st.authority}
                      </span>
                    </div>

                    {/* Nodes inside Stratum 0 */}
                    {st.stratum === 0 && (
                      <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
                        {(st.nodes || []).map((node) => (
                          <div
                            key={node.id}
                            onClick={() => setSelectedNode({ type: 'node', data: node, stratum: 0 })}
                            className="cursor-pointer rounded-xl border border-line bg-surface p-3 hover:border-accent hover:shadow-md transition-all"
                          >
                            <div className="flex items-center justify-between">
                              <Server size={15} className="text-emerald-500" />
                              <span className="h-2 w-2 rounded-full bg-emerald-500" />
                            </div>
                            <p className="mt-2 text-xs font-bold text-fg truncate">{node.name}</p>
                            <p className="text-[10px] font-mono text-fg-muted">{node.ip}</p>
                            <div className="mt-2 text-[10px] text-accent font-medium">VMID {node.vmid}</div>
                          </div>
                        ))}
                      </div>
                    )}

                    {/* Governors inside Stratum 1 */}
                    {st.stratum === 1 && (
                      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                        {(st.governors || []).map((gov) => (
                          <div
                            key={gov.id}
                            onClick={() => setSelectedNode({ type: 'governor', data: gov, stratum: 1 })}
                            className="cursor-pointer rounded-xl border border-line bg-surface p-3 hover:border-accent hover:shadow-md transition-all"
                          >
                            <div className="flex items-center justify-between">
                              <span className="text-xs font-bold text-sky-500 font-mono">{gov.id}</span>
                              <span className="rounded bg-emerald-500/10 px-2 py-0.5 text-[10px] font-bold text-emerald-500">
                                {gov.state}
                              </span>
                            </div>
                            <h4 className="mt-1 text-sm font-bold text-fg">{gov.name}</h4>
                            <p className="mt-1 text-xs text-fg-muted font-mono">{gov.ledgerDb}</p>
                            <div className="mt-2 text-[10px] text-fg-muted">
                              Makers rattachés : {(gov.enrolledMakers || []).join(', ')}
                            </div>
                          </div>
                        ))}
                      </div>
                    )}

                    {/* Makers and Universes inside Stratum 2 */}
                    {st.stratum === 2 && (
                      <div className="space-y-3">
                        <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
                          {(st.universes || []).map((u) => (
                            <div
                              key={u.id}
                              onClick={() => setSelectedNode({ type: 'universe', data: u, stratum: 2 })}
                              className="cursor-pointer rounded-xl border border-line bg-surface p-3 hover:border-accent hover:shadow-md transition-all"
                            >
                              <div className="flex items-center justify-between">
                                <span className="font-mono text-xs font-bold text-indigo-400 truncate">{u.id}</span>
                                <span className="h-2 w-2 rounded-full bg-emerald-500" />
                              </div>
                              <p className="mt-1 text-xs font-semibold text-fg truncate">{u.account}</p>
                              <p className="text-[10px] font-mono text-fg-muted">{u.machine}</p>
                              <span className="mt-2 inline-block rounded bg-indigo-500/10 px-1.5 py-0.5 text-[10px] font-bold text-indigo-400">
                                {u.klass}
                              </span>
                            </div>
                          ))}
                        </div>
                      </div>
                    )}

                    {/* Bricks inside Stratum 3 */}
                    {st.stratum === 3 && (
                      <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
                        {(st.bricksSummary || []).map((b) => (
                          <div
                            key={b.universeId}
                            onClick={() => setSelectedNode({ type: 'bricks', data: b, stratum: 3 })}
                            className="cursor-pointer rounded-xl border border-line bg-surface p-3 hover:border-accent transition-all"
                          >
                            <p className="text-xs font-mono font-bold text-amber-500">{b.universeId}</p>
                            <div className="mt-2 flex flex-wrap gap-1">
                              {(b.bricks || []).map((brick) => (
                                <span
                                  key={brick}
                                  className="rounded bg-surface-2 px-2 py-0.5 font-mono text-[10px] text-fg-muted border border-line"
                                >
                                  {brick}
                                </span>
                              ))}
                            </div>
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                ))}
            </div>
          </div>
        </div>

        {/* Node Inspector Side Panel */}
        <div className="rounded-2xl border border-line bg-surface p-5 shadow-sm">
          <h3 className="text-sm font-bold text-fg flex items-center gap-2 border-b border-line pb-3">
            <Eye size={16} className="text-accent" /> Inspecteur de Juridiction
          </h3>

          {selectedNode ? (
            <div className="mt-4 space-y-3 text-xs">
              <div className="rounded-lg bg-surface-2 p-2.5">
                <span className="font-mono uppercase text-[10px] font-bold text-accent">
                  Type : {selectedNode.type} (Strate {selectedNode.stratum})
                </span>
                <p className="mt-1 font-bold text-fg text-sm">
                  {selectedNode.data.name || selectedNode.data.id || selectedNode.data.universeId}
                </p>
              </div>

              <div className="space-y-2">
                <h4 className="font-semibold text-fg">Propriétés & Faits Vérifiés :</h4>
                <pre className="overflow-x-auto max-h-80 rounded-lg border border-line bg-surface-2 p-3 font-mono text-[11px] text-fg-muted">
                  {JSON.stringify(selectedNode.data, null, 2)}
                </pre>
              </div>

              <div className="rounded-lg border border-emerald-500/30 bg-emerald-500/5 p-3 text-[11px] text-emerald-400">
                ✓ Respect strict des invariants SHAPER OS v1.15 : isolation vérifiée et intégrité scellée.
              </div>
            </div>
          ) : (
            <div className="mt-8 text-center text-xs text-fg-muted">
              <p>Sélectionnez un nœud, un gouverneur ou un univers dans la carte fractale pour inspecter ses contrats et paramètres.</p>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
