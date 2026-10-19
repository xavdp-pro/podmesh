import React from 'react'

/**
 * Composants SVG Haute Définition respectant la charte graphique officielle SHAPER OS (Atelier Vivant).
 * Thème : Dark Canvas #0c1512, Indigo/Violet #4f46e5 / #6366f1, Cyan #38bdf8, Emerald #10b981, Amber #f59e0b.
 */

export function StrataSvgDiagram({ activeStrate = '0', onSelectStrate }) {
  const isS0 = activeStrate === '0'
  const isS1 = activeStrate === '1'
  const isS2 = activeStrate === '2'
  const isS3 = activeStrate === '3'

  return (
    <div className="w-full overflow-hidden rounded-2xl border border-line bg-surface/50 p-2 sm:p-4 backdrop-blur-md">
      <svg
        viewBox="0 0 960 440"
        className="w-full h-auto select-none"
        xmlns="http://www.w3.org/2000/svg"
      >
        <defs>
          <linearGradient id="bgGridGrad" x1="0%" y1="0%" x2="100%" y2="100%">
            <stop offset="0%" stopColor="#0c1512" />
            <stop offset="100%" stopColor="#142019" />
          </linearGradient>

          <pattern id="gridPattern" width="30" height="30" patternUnits="userSpaceOnUse">
            <path d="M 30 0 L 0 0 0 30" fill="none" stroke="#33463b" strokeWidth="0.6" strokeOpacity="0.4" />
          </pattern>

          {/* Gradients pour les Strates */}
          <linearGradient id="gradS0" x1="0%" y1="0%" x2="100%" y2="0%">
            <stop offset="0%" stopColor="#6366f1" stopOpacity="0.25" />
            <stop offset="100%" stopColor="#4f46e5" stopOpacity="0.1" />
          </linearGradient>
          <linearGradient id="gradS1" x1="0%" y1="0%" x2="100%" y2="0%">
            <stop offset="0%" stopColor="#0ea5e9" stopOpacity="0.25" />
            <stop offset="100%" stopColor="#0284c7" stopOpacity="0.1" />
          </linearGradient>
          <linearGradient id="gradS2" x1="0%" y1="0%" x2="100%" y2="0%">
            <stop offset="0%" stopColor="#10b981" stopOpacity="0.25" />
            <stop offset="100%" stopColor="#059669" stopOpacity="0.1" />
          </linearGradient>
          <linearGradient id="gradS3" x1="0%" y1="0%" x2="100%" y2="0%">
            <stop offset="0%" stopColor="#f59e0b" stopOpacity="0.25" />
            <stop offset="100%" stopColor="#d97706" stopOpacity="0.1" />
          </linearGradient>

          {/* Filtres néon */}
          <filter id="neonGlow" x="-20%" y="-20%" width="140%" height="140%">
            <feGaussianBlur stdDeviation="4" result="blur" />
            <feComposite in="SourceGraphic" in2="blur" operator="over" />
          </filter>
        </defs>

        {/* Fond avec motif grille */}
        <rect width="960" height="440" rx="16" fill="url(#bgGridGrad)" />
        <rect width="960" height="440" rx="16" fill="url(#gridPattern)" />

        {/* Titre Shaper OS */}
        <text x="32" y="36" fill="#94a3b8" fontSize="12" fontWeight="700" letterSpacing="1.5">
          SHAPER OS v1.15 · SÉPARATION DES 4 STRATES DE JURIDICTION
        </text>

        {/* ================= STRATE 3 ================= */}
        <g
          className="cursor-pointer transition-all duration-200"
          onClick={() => onSelectStrate && onSelectStrate('3')}
        >
          <rect
            x="32"
            y="54"
            width="896"
            height="72"
            rx="12"
            fill="url(#gradS3)"
            stroke={isS3 ? '#f59e0b' : '#334155'}
            strokeWidth={isS3 ? '2.5' : '1'}
            filter={isS3 ? 'url(#neonGlow)' : undefined}
          />
          <circle cx="56" cy="90" r="14" fill="#f59e0b" fillOpacity="0.2" stroke="#f59e0b" strokeWidth="1.5" />
          <text x="56" y="94" textAnchor="middle" fill="#f59e0b" fontSize="12" fontWeight="900">S3</text>
          <text x="82" y="82" fill="#f8fafc" fontSize="14" fontWeight="800">
            STRATE 3 — BRICKS & UNIVERS CLIENTS DÉPLOYÉS
          </text>
          <text x="82" y="104" fill="#94a3b8" fontSize="11">
            Conteneurs isolés sous userns-v1 + cgroups v2 (Boutique WooCommerce, Vox Softphone Asterisk, DB client).
          </text>
          <rect x="760" y="72" width="150" height="34" rx="8" fill="#0f172a" stroke="#f59e0b" strokeWidth="1" />
          <text x="835" y="93" textAnchor="middle" fill="#fbbf24" fontSize="11" fontWeight="700">
            Confinement Total
          </text>
        </g>

        {/* Flèche Lien S3 <-> S2 */}
        <path d="M 480 126 L 480 148" stroke="#64748b" strokeWidth="1.5" strokeDasharray="4 4" />
        <text x="490" y="142" fill="#64748b" fontSize="10">Recette scellée (6 args positionnels)</text>

        {/* ================= STRATE 2 ================= */}
        <g
          className="cursor-pointer transition-all duration-200"
          onClick={() => onSelectStrate && onSelectStrate('2')}
        >
          <rect
            x="32"
            y="148"
            width="896"
            height="72"
            rx="12"
            fill="url(#gradS2)"
            stroke={isS2 ? '#10b981' : '#334155'}
            strokeWidth={isS2 ? '2.5' : '1'}
            filter={isS2 ? 'url(#neonGlow)' : undefined}
          />
          <circle cx="56" cy="184" r="14" fill="#10b981" fillOpacity="0.2" stroke="#10b981" strokeWidth="1.5" />
          <text x="56" y="188" textAnchor="middle" fill="#10b981" fontSize="12" fontWeight="900">S2</text>
          <text x="82" y="176" fill="#f8fafc" fontSize="14" fontWeight="800">
            STRATE 2 — MAKERS SPÉCIALISATEURS (OUVRIERS D'EXÉCUTION)
          </text>
          <text x="82" y="198" fill="#94a3b8" fontSize="11">
            N'écoute sur AUCUN port. Polling sortant vers son Gouverneur. Vérifie les SHA-256, exécute stamp/reap/validate/adopt.
          </text>
          <rect x="760" y="166" width="150" height="34" rx="8" fill="#0f172a" stroke="#10b981" strokeWidth="1" />
          <text x="835" y="187" textAnchor="middle" fill="#34d399" fontSize="11" fontWeight="700">
            Zéro Port Écouté
          </text>
        </g>

        {/* Flèche Lien S2 <-> S1 */}
        <path d="M 480 220 L 480 242" stroke="#0ea5e9" strokeWidth="2" />
        <polygon points="476,242 484,242 480,248" fill="#0ea5e9" />
        <text x="490" y="236" fill="#38bdf8" fontSize="10" fontWeight="600">POST /api/makers/poll (Sortant)</text>

        {/* ================= STRATE 1 ================= */}
        <g
          className="cursor-pointer transition-all duration-200"
          onClick={() => onSelectStrate && onSelectStrate('1')}
        >
          <rect
            x="32"
            y="242"
            width="896"
            height="72"
            rx="12"
            fill="url(#gradS1)"
            stroke={isS1 ? '#0ea5e9' : '#334155'}
            strokeWidth={isS1 ? '2.5' : '1'}
            filter={isS1 ? 'url(#neonGlow)' : undefined}
          />
          <circle cx="56" cy="278" r="14" fill="#0ea5e9" fillOpacity="0.2" stroke="#0ea5e9" strokeWidth="1.5" />
          <text x="56" y="282" textAnchor="middle" fill="#0ea5e9" fontSize="12" fontWeight="900">S1</text>
          <text x="82" y="270" fill="#f8fafc" fontSize="14" fontWeight="800">
            STRATE 1 — GOUVERNEURS DE FRACTALES (TÊTES DE FLOTTE SAAS)
          </text>
          <text x="82" y="292" fill="#94a3b8" fontSize="11">
            Gouverne par l'écriture (&quot;Govern by writing&quot;) dans son ledger MariaDB privé : DESIRED → RECONCILING → PURRING.
          </text>
          <rect x="760" y="260" width="150" height="34" rx="8" fill="#0f172a" stroke="#0ea5e9" strokeWidth="1" />
          <text x="835" y="281" textAnchor="middle" fill="#38bdf8" fontSize="11" fontWeight="700">
            Govern by Writing
          </text>
        </g>

        {/* Flèche Lien S1 <-> S0 */}
        <path d="M 480 314 L 480 336" stroke="#818cf8" strokeWidth="2" strokeDasharray="3 3" />
        <text x="490" y="330" fill="#a5b4fc" fontSize="10">Surveillance et Sauvegarde par en-dessous</text>

        {/* ================= STRATE 0 ================= */}
        <g
          className="cursor-pointer transition-all duration-200"
          onClick={() => onSelectStrate && onSelectStrate('0')}
        >
          <rect
            x="32"
            y="336"
            width="896"
            height="76"
            rx="12"
            fill="url(#gradS0)"
            stroke={isS0 ? '#6366f1' : '#334155'}
            strokeWidth={isS0 ? '2.5' : '1'}
            filter={isS0 ? 'url(#neonGlow)' : undefined}
          />
          <circle cx="56" cy="374" r="14" fill="#6366f1" fillOpacity="0.2" stroke="#6366f1" strokeWidth="1.5" />
          <text x="56" y="378" textAnchor="middle" fill="#6366f1" fontSize="12" fontWeight="900">S0</text>
          <text x="82" y="364" fill="#f8fafc" fontSize="14" fontWeight="800">
            STRATE 0 — MANAGER PODMESH SOUVERAIN & INFRASTRUCTURE
          </text>
          <text x="82" y="386" fill="#94a3b8" fontSize="11">
            Pouvoir hôte direct : Podman, Cgroups v2, ZFS/Btrfs, MeshFS, PMBS Op 10, Déplacement à chaud en 42ms sans intrusion.
          </text>
          <rect x="760" y="354" width="150" height="34" rx="8" fill="#0f172a" stroke="#6366f1" strokeWidth="1" />
          <text x="835" y="375" textAnchor="middle" fill="#a5b4fc" fontSize="11" fontWeight="700">
            Souverain Hôte
          </text>
        </g>
      </svg>
    </div>
  )
}

export function MeshFsSvgDiagram({ activeDriver = 'zfs' }) {
  return (
    <div className="w-full overflow-hidden rounded-2xl border border-line bg-surface/50 p-2 sm:p-4 backdrop-blur-md">
      <svg
        viewBox="0 0 960 440"
        className="w-full h-auto select-none"
        xmlns="http://www.w3.org/2000/svg"
      >
        <defs>
          <linearGradient id="mfsGradBg" x1="0%" y1="0%" x2="100%" y2="100%">
            <stop offset="0%" stopColor="#0c1512" />
            <stop offset="100%" stopColor="#16223a" />
          </linearGradient>

          <pattern id="mfsGrid" width="24" height="24" patternUnits="userSpaceOnUse">
            <path d="M 24 0 L 0 0 0 24" fill="none" stroke="#33463b" strokeWidth="0.5" strokeOpacity="0.4" />
          </pattern>
        </defs>

        <rect width="960" height="440" rx="16" fill="url(#mfsGradBg)" />
        <rect width="960" height="440" rx="16" fill="url(#mfsGrid)" />

        <text x="32" y="34" fill="#94a3b8" fontSize="12" fontWeight="700" letterSpacing="1.5">
          MESHFS — MOTEUR DE STOCKAGE DISTRIBUÉ DÉDUPLIQUÉ & ARBRE DE MERKLE
        </text>

        {/* NIVEAU 1 : Fichiers Entrants */}
        <rect x="32" y="52" width="260" height="110" rx="10" fill="#142019" stroke="#334155" strokeWidth="1" />
        <text x="48" y="74" fill="#f8fafc" fontSize="13" fontWeight="800">1. Découpage en Chunks CAS</text>
        <text x="48" y="94" fill="#94a3b8" fontSize="11">Fichiers volumineux ou disques VM</text>
        <g transform="translate(48, 106)">
          <rect x="0" y="0" width="60" height="28" rx="6" fill="#3b82f6" fillOpacity="0.2" stroke="#3b82f6" />
          <text x="30" y="18" textAnchor="middle" fill="#60a5fa" fontSize="10" fontWeight="700">4 Mo #1</text>
          <rect x="70" y="0" width="60" height="28" rx="6" fill="#3b82f6" fillOpacity="0.2" stroke="#3b82f6" />
          <text x="100" y="18" textAnchor="middle" fill="#60a5fa" fontSize="10" fontWeight="700">4 Mo #2</text>
          <rect x="140" y="0" width="60" height="28" rx="6" fill="#3b82f6" fillOpacity="0.2" stroke="#3b82f6" />
          <text x="170" y="18" textAnchor="middle" fill="#60a5fa" fontSize="10" fontWeight="700">4 Mo #3</text>
        </g>
        <text x="48" y="152" fill="#64748b" fontSize="10">Hachage SHA-256 SIMD matériel</text>

        {/* Flèche vers Merkle Tree */}
        <path d="M 292 107 L 350 107" stroke="#38bdf8" strokeWidth="2" strokeDasharray="4 4" />
        <polygon points="350,103 358,107 350,111" fill="#38bdf8" />

        {/* NIVEAU 2 : Arbre d'Inodes de Merkle */}
        <rect x="360" y="52" width="310" height="150" rx="10" fill="#142019" stroke="#38bdf8" strokeWidth="1.5" />
        <text x="376" y="74" fill="#38bdf8" fontSize="13" fontWeight="800">2. Arbre d'Inodes de Merkle (CoW)</text>
        
        {/* Nœuds Merkle */}
        <g transform="translate(376, 88)">
          <rect x="90" y="0" width="110" height="24" rx="6" fill="#0284c7" fillOpacity="0.3" stroke="#38bdf8" />
          <text x="145" y="16" textAnchor="middle" fill="#e0f2fe" fontSize="10" fontWeight="700">MerkleRoot SHA-256</text>

          <path d="M 120 24 L 60 48 M 170 24 L 230 48" stroke="#38bdf8" strokeWidth="1.2" />

          <rect x="15" y="48" width="90" height="22" rx="4" fill="#0f172a" stroke="#64748b" />
          <text x="60" y="63" textAnchor="middle" fill="#94a3b8" fontSize="9">/etc Inode Node</text>

          <rect x="185" y="48" width="90" height="22" rx="4" fill="#0f172a" stroke="#64748b" />
          <text x="230" y="63" textAnchor="middle" fill="#94a3b8" fontSize="9">/var Inode Node</text>

          <text x="145" y="100" textAnchor="middle" fill="#34d399" fontSize="11" fontWeight="700">
            Snapshots instantanés O(1) &amp; Merkle-Diff
          </text>
        </g>

        {/* Pont direct vers PMBS et Proxmox */}
        <rect x="690" y="52" width="238" height="150" rx="10" fill="#142019" stroke="#10b981" strokeWidth="1.5" />
        <text x="706" y="74" fill="#10b981" fontSize="13" fontWeight="800">3. Connecteurs Natifs</text>

        <rect x="706" y="88" width="206" height="44" rx="8" fill="#0f172a" stroke="#059669" />
        <text x="718" y="106" fill="#f8fafc" fontSize="11" fontWeight="700">PodMesh Backup Server</text>
        <text x="718" y="122" fill="#94a3b8" fontSize="9">Export incrémental sans re-découpage</text>

        <rect x="706" y="142" width="206" height="44" rx="8" fill="#0f172a" stroke="#6366f1" />
        <text x="718" y="160" fill="#f8fafc" fontSize="11" fontWeight="700">Proxmox VE Storage Plugin</text>
        <text x="718" y="176" fill="#94a3b8" fontSize="9">PVE::Storage::Custom::MeshFSPlugin</text>

        {/* NIVEAU 3 : Multi-Moteurs Hôte */}
        <text x="32" y="240" fill="#94a3b8" fontSize="12" fontWeight="700" letterSpacing="1.2">
          MULTI-COMPATIBILITÉ DES MOTEURS SOUS-JACENTS (AUTO-DÉTECTÉS PAR MESHFSD)
        </text>

        <g transform="translate(32, 256)">
          {/* ZFS */}
          <rect x="0" y="0" width="212" height="144" rx="10" fill="#142019" stroke={activeDriver === 'zfs' ? '#6366f1' : '#334155'} strokeWidth="1.5" />
          <text x="16" y="28" fill="#a5b4fc" fontSize="14" fontWeight="800">ZFS Dataset</text>
          <text x="16" y="48" fill="#94a3b8" fontSize="10">Idéal sur Proxmox VE (s0)</text>
          <circle cx="26" cy="72" r="4" fill="#6366f1" />
          <text x="36" y="75" fill="#f8fafc" fontSize="10">Compression ZSTD native</text>
          <circle cx="26" cy="94" r="4" fill="#6366f1" />
          <text x="36" y="97" fill="#f8fafc" fontSize="10">ARC RAM Cache ultra-rapide</text>
          <circle cx="26" cy="116" r="4" fill="#6366f1" />
          <text x="36" y="119" fill="#f8fafc" fontSize="10">Checksums de bout en bout</text>

          {/* BTRFS */}
          <rect x="228" y="0" width="212" height="144" rx="10" fill="#142019" stroke={activeDriver === 'btrfs' ? '#0ea5e9' : '#334155'} strokeWidth="1.5" />
          <text x="244" y="28" fill="#38bdf8" fontSize="14" fontWeight="800">Btrfs Subvolume</text>
          <text x="244" y="48" fill="#94a3b8" fontSize="10">Idéal sur clusters multi-nœuds HA</text>
          <circle cx="254" cy="72" r="4" fill="#0ea5e9" />
          <text x="264" y="75" fill="#f8fafc" fontSize="10">Reflink CoW instantané</text>
          <circle cx="254" cy="94" r="4" fill="#0ea5e9" />
          <text x="264" y="97" fill="#f8fafc" fontSize="10">Snapshots subvol en 1ms</text>
          <circle cx="254" cy="116" r="4" fill="#0ea5e9" />
          <text x="264" y="119" fill="#f8fafc" fontSize="10">Zéro duplication d&apos;octets</text>

          {/* LVM THIN */}
          <rect x="456" y="0" width="212" height="144" rx="10" fill="#142019" stroke={activeDriver === 'lvm' ? '#10b981' : '#334155'} strokeWidth="1.5" />
          <text x="472" y="28" fill="#34d399" fontSize="14" fontWeight="800">LVM Thin-Pool</text>
          <text x="472" y="48" fill="#94a3b8" fontSize="10">Idéal Datacenters SAN / NVMe</text>
          <circle cx="482" cy="72" r="4" fill="#10b981" />
          <text x="492" y="75" fill="#f8fafc" fontSize="10">Quotas stricts au bloc</text>
          <circle cx="482" cy="94" r="4" fill="#10b981" />
          <text x="492" y="97" fill="#f8fafc" fontSize="10">Sur-allocation sécurisée</text>
          <circle cx="482" cy="116" r="4" fill="#10b981" />
          <text x="492" y="119" fill="#f8fafc" fontSize="10">Expansion à chaud</text>

          {/* POSIX DIR FALLBACK */}
          <rect x="684" y="0" width="212" height="144" rx="10" fill="#142019" stroke="#334155" strokeWidth="1.5" />
          <text x="700" y="28" fill="#f59e0b" fontSize="14" fontWeight="800">POSIX Dir Fallback</text>
          <text x="700" y="48" fill="#94a3b8" fontSize="10">N&apos;importe quel Linux (ext4, xfs)</text>
          <circle cx="710" cy="72" r="4" fill="#f59e0b" />
          <text x="720" y="75" fill="#f8fafc" fontSize="10">Sparse Files (fallocate)</text>
          <circle cx="710" cy="94" r="4" fill="#f59e0b" />
          <text x="720" y="97" fill="#f8fafc" fontSize="10">Hardlinks automatiques</text>
          <circle cx="710" cy="116" r="4" fill="#f59e0b" />
          <text x="720" y="119" fill="#f8fafc" fontSize="10">Tourne sur TOUT serveur nu</text>
        </g>
      </svg>
    </div>
  )
}

export function FederationDualLabSvgDiagram({ activeSite = 'all' }) {
  return (
    <div className="w-full overflow-hidden rounded-2xl border border-line bg-surface/50 p-2 sm:p-4 backdrop-blur-md">
      <svg
        viewBox="0 0 960 440"
        className="w-full h-auto select-none"
        xmlns="http://www.w3.org/2000/svg"
      >
        <defs>
          <linearGradient id="fedBg" x1="0%" y1="0%" x2="100%" y2="100%">
            <stop offset="0%" stopColor="#0c1512" />
            <stop offset="100%" stopColor="#142019" />
          </linearGradient>

          <pattern id="fedGrid" width="28" height="28" patternUnits="userSpaceOnUse">
            <path d="M 28 0 L 0 0 0 28" fill="none" stroke="#33463b" strokeWidth="0.5" strokeOpacity="0.4" />
          </pattern>
        </defs>

        <rect width="960" height="440" rx="16" fill="url(#fedBg)" />
        <rect width="960" height="440" rx="16" fill="url(#fedGrid)" />

        <text x="32" y="34" fill="#94a3b8" fontSize="12" fontWeight="700" letterSpacing="1.5">
          TOPOLOGIE DE RÉFÉRENCE · FÉDÉRATION MULTI-SITES &amp; HAUTE DISPONIBILITÉ (N RÉGIONS)
        </text>

        {/* ================= CLUSTER 1 : REGION PRIMAIRE ================= */}
        <g transform="translate(32, 54)">
          <rect width="420" height="340" rx="14" fill="#142019" stroke="#4f46e5" strokeWidth="1.5" />
          <rect x="0" y="0" width="420" height="42" rx="14" fill="#4f46e5" fillOpacity="0.15" />
          <text x="20" y="26" fill="#a5b4fc" fontSize="14" fontWeight="800">
            RÉGION PRIMAIRE (Site A · Quorum Actif)
          </text>
          <text x="320" y="26" fill="#10b981" fontSize="11" fontWeight="700">Actif Ingress</text>

          {/* Managers Quorum */}
          <rect x="20" y="56" width="380" height="82" rx="8" fill="#0f172a" stroke="#334155" />
          <text x="36" y="78" fill="#f8fafc" fontSize="12" fontWeight="700">Contrôleurs PodMesh (Consensus Distribué Ed25519)</text>
          <rect x="36" y="90" width="110" height="34" rx="6" fill="#1e1b4b" stroke="#6366f1" />
          <text x="91" y="107" textAnchor="middle" fill="#c7d2fe" fontSize="10" fontWeight="700">Leader Actif</text>
          <text x="91" y="119" textAnchor="middle" fill="#818cf8" fontSize="8">Contrôleur 01</text>

          <rect x="156" y="90" width="110" height="34" rx="6" fill="#1e1b4b" stroke="#6366f1" />
          <text x="211" y="107" textAnchor="middle" fill="#c7d2fe" fontSize="10" fontWeight="700">Voter Quorum</text>
          <text x="211" y="119" textAnchor="middle" fill="#818cf8" fontSize="8">Contrôleur 02</text>

          <rect x="276" y="90" width="110" height="34" rx="6" fill="#1e1b4b" stroke="#6366f1" />
          <text x="331" y="107" textAnchor="middle" fill="#c7d2fe" fontSize="10" fontWeight="700">Nœud Arbitre</text>
          <text x="331" y="119" textAnchor="middle" fill="#818cf8" fontSize="8">Témoin Consensus</text>

          {/* Node d'Exécution & MeshFS */}
          <rect x="20" y="150" width="380" height="74" rx="8" fill="#0f172a" stroke="#334155" />
          <text x="36" y="172" fill="#f8fafc" fontSize="12" fontWeight="700">Nœuds d&apos;Exécution &amp; Calcul (Bare-Metal / KVM / Cloud)</text>
          <text x="36" y="190" fill="#94a3b8" fontSize="10">Volume Btrfs CoW · Kernel 6.12 · Maker Spécialisateur enrôlé</text>
          <text x="36" y="208" fill="#38bdf8" fontSize="10" fontWeight="600">MeshFS Local ChunkStore : /var/lib/meshfs/chunks</text>

          {/* PMBS Local Datastore */}
          <rect x="20" y="236" width="380" height="84" rx="8" fill="#0f172a" stroke="#10b981" />
          <text x="36" y="258" fill="#34d399" fontSize="12" fontWeight="700">PMBS Datastore (Stockage Dédupliqué Local)</text>
          <text x="36" y="276" fill="#94a3b8" fontSize="10">Facteur de déduplication : 4.8x · Stockage delta vérifié</text>
          <text x="36" y="296" fill="#f59e0b" fontSize="10" fontWeight="600">Preuve Op 10 : Passée (Dernier snapshot validé)</text>
        </g>

        {/* ================= LIEN CENTRAL REPLICATION & VPN ================= */}
        <g transform="translate(460, 160)">
          <line x1="0" y1="30" x2="40" y2="30" stroke="#38bdf8" strokeWidth="2" strokeDasharray="4 4" />
          <rect x="0" y="60" width="40" height="40" rx="8" fill="#0f172a" stroke="#38bdf8" strokeWidth="1" />
          <text x="20" y="84" textAnchor="middle" fill="#38bdf8" fontSize="10" fontWeight="700">SYNC</text>
          <line x1="0" y1="120" x2="40" y2="120" stroke="#10b981" strokeWidth="2" />
        </g>

        {/* ================= CLUSTER 2 : REGION SECONDAIRE / MIROIR ================= */}
        <g transform="translate(508, 54)">
          <rect width="420" height="340" rx="14" fill="#142019" stroke="#0ea5e9" strokeWidth="1.5" />
          <rect x="0" y="0" width="420" height="42" rx="14" fill="#0ea5e9" fillOpacity="0.15" />
          <text x="20" y="26" fill="#7dd3fc" fontSize="14" fontWeight="800">
            RÉGION DISTANTE (Site B · Miroir Répliqué)
          </text>
          <text x="320" y="26" fill="#38bdf8" fontSize="11" fontWeight="700">Miroir HA</text>

          {/* Managers Quorum Miroir */}
          <rect x="20" y="56" width="380" height="82" rx="8" fill="#0f172a" stroke="#334155" />
          <text x="36" y="78" fill="#f8fafc" fontSize="12" fontWeight="700">Contrôleurs PodMesh Miroirs (Sync Continue Ed25519)</text>
          <rect x="36" y="90" width="110" height="34" rx="6" fill="#082f49" stroke="#0ea5e9" />
          <text x="91" y="107" textAnchor="middle" fill="#bae6fd" fontSize="10" fontWeight="700">Miroir 01</text>
          <text x="91" y="119" textAnchor="middle" fill="#38bdf8" fontSize="8">Réplique HA</text>

          <rect x="156" y="90" width="110" height="34" rx="6" fill="#082f49" stroke="#0ea5e9" />
          <text x="211" y="107" textAnchor="middle" fill="#bae6fd" fontSize="10" fontWeight="700">Miroir 02</text>
          <text x="211" y="119" textAnchor="middle" fill="#38bdf8" fontSize="8">Réplique HA</text>

          <rect x="276" y="90" width="110" height="34" rx="6" fill="#082f49" stroke="#0ea5e9" />
          <text x="331" y="107" textAnchor="middle" fill="#bae6fd" fontSize="10" fontWeight="700">Arbitre Miroir</text>
          <text x="331" y="119" textAnchor="middle" fill="#38bdf8" fontSize="8">Témoin HA</text>

          {/* Node d'Exécution & MeshFS & Proxmox Host */}
          <rect x="20" y="150" width="380" height="74" rx="8" fill="#0f172a" stroke="#334155" />
          <text x="36" y="172" fill="#f8fafc" fontSize="12" fontWeight="700">Hyperviseur Proxmox VE &amp; Nœuds Compute</text>
          <text x="36" y="190" fill="#94a3b8" fontSize="10">ZFS Pool · Plugin Proxmox PVE::Storage::Custom::MeshFSPlugin</text>
          <text x="36" y="208" fill="#34d399" fontSize="10" fontWeight="600">MeshFS FUSE &amp; QEMU VM Raw/Qcow2 Disks supportés</text>

          {/* PMBS Remote Datastore */}
          <rect x="20" y="236" width="380" height="84" rx="8" fill="#0f172a" stroke="#10b981" />
          <text x="36" y="258" fill="#34d399" fontSize="12" fontWeight="700">PMBS Datastore Distant (Miroir de Sauvegarde)</text>
          <text x="36" y="276" fill="#94a3b8" fontSize="10">Remote Sync Target · Datastore Dédupliqué · Déduplication 4.8x</text>
          <text x="36" y="296" fill="#38bdf8" fontSize="10" fontWeight="600">Cible de validation Sandbox Boot Restore (1.1s)</text>
        </g>
      </svg>
    </div>
  )
}
