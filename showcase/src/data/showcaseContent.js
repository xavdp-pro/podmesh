/**
 * High-definition technical showcase content in French, English, and Spanish.
 * Strictly adheres to SHAPER OS v1.15 and PodMesh Phase 2 architecture.
 */

export const showcaseContent = {
  fr: {
    github: {
      title: "Dépôts Officiels sur GitHub",
      podmeshUrl: "https://github.com/xavdp-pro/podmesh",
      shaperUrl: "https://github.com/xavdp-pro/SHAPER-OS-V1.13",
      podmeshLabel: "Dépôt GitHub PodMesh",
      shaperLabel: "Dépôt GitHub Shaper OS",
    },
    tandem: {
      badge: "Doctrine Shaper OS • Tandem Humain ⇄ Agent IA",
      title: "Conçu Nativement pour le Tandem Symbiotique Humain & Agent IA",
      lead: "PodMesh et Shaper OS ne sont ni des outils manuels rigides, ni des boîtes noires autonomes incontrôlables. L’architecture est forgée pour un co-pilotage où l’humain fixe l’intention stratégique et l’agent IA garantit la rigueur mathématique et l’auto-réparation en temps réel.",
      levels: [
        {
          id: "human",
          name: "Humain & Décideur (Non-Geek)",
          role: "Intention & Gouvernance Souveraine",
          summary: "Comprendre les bénéfices concrets sans se noyer dans le jargon : coûts réduits, zéro dépendance aux GAFAM, résilience maximale.",
          badge: "Vision Métier"
        },
        {
          id: "geek",
          name: "Ingénieur Système (Geek)",
          role: "Noyau Linux, CAS & Protocoles",
          summary: "Les entrailles techniques du système : Merkle DAG, chunks CAS 4MB, userns UID mapping, cgroups v2, Quorum Ed25519.",
          badge: "Spécifications Systèmes"
        },
        {
          id: "agent",
          name: "Agent IA Opérateur (Machine)",
          role: "Invariants Formels & API Déterministe",
          summary: "Les règles absolues pour l’IA : zéro ambiguïté, validation formelle par hash SHA-256, gouvernance par l’écriture et auto-guérison.",
          badge: "Contrats Machine"
        }
      ]
    },
    hero: {
      badge: 'Architecture Officielle • SHAPER OS v1.15',
      title: 'PodMesh Cloud OS & Fédération Multi-Fractales',
      subtitle:
        'Plateforme d’orchestration souveraine, stockage distribué MeshFS et sauvegarde immuable PMBS pour infrastructures critiques et Proxmox VE.',
      btnMeshfs: 'Spécifications MeshFS',
      btnQuickstart: 'Démarrage Rapide CLI',
      btnExplore: 'Explorer l’Architecture',
    },
    metrics: [
      { label: 'Consensus & Quorum', value: 'Zéro Split-Brain', sub: 'Signatures cryptographiques Ed25519' },
      { label: 'Haute Disponibilité', value: '< 50 ms', sub: 'Bascule CoW Btrfs & ZFS' },
      { label: 'Preuve de Survie Op 10', value: '100% Qualifiée', sub: 'Validation Sandbox Boot Restore' },
      { label: 'Empreinte RAM MeshFS', value: '< 64 MiB', sub: 'vs 4 GiB+ par OSD Ceph' },
    ],
    navTabs: {
      podmesh: 'PodMesh Engine',
      pmbs: 'PMBS Backup Server',
      meshfs: 'MeshFS Distribué',
      proxmox: 'Plugin Proxmox VE',
      downloads: 'Téléchargements & CLI',
    },
    podmesh: {
      perspectives: {
        human: {
          title: "Vision Humain & Décideur (En clair)",
          subtitle: "Pourquoi PodMesh libère votre entreprise de la dépendance aux GAFAM",
          points: [
            { label: "Le Problème Résolu", text: "Comment exécuter vos applications métiers sur vos propres serveurs sans payer de coûteuses licences ni embaucher une armée d’ingénieurs certifiés AWS/K8s ?" },
            { label: "La Métaphore Simple", text: "Comme un chef d’orchestre invisible : si un serveur prend feu, vos applications déménagent sur une autre machine en 42 millisecondes sans que vos utilisateurs ne s’en aperçoivent." },
            { label: "Le Gain Direct", text: "Facture de serveurs divisée par 10, zéro lock-in américain, conformité RGPD souveraine et sérénité opérationnelle." }
          ]
        },
        geek: {
          title: "Sous le Capot (Spécifications Geek & Noyau Linux)",
          subtitle: "Orchestration fractale sans etcd monolithique et isolation userns",
          points: [
            { label: "Architecture", text: "Séparation hermétique des 4 Strates (Loi Shaper OS v1.15). Le père n’entre jamais dans les entrailles du fils." },
            { label: "Sécurité Noyau", text: "Confinement par userns (UID remapping 100000..165535) et cgroups v2 (cpu, memory, io, pids). Root dans le conteneur = utilisateur non privilégié sur l’hôte." },
            { label: "Haute Disponibilité", text: "Consensus distribué tolérant aux pannes (2F+1) signé Ed25519 avec consensus par époque et relocalisation à chaud via snapshots Btrfs/ZFS reflink en < 50ms." }
          ]
        },
        agent: {
          title: "Pilotage par Agent IA (Contrats Formels & Machine)",
          subtitle: "Invariants stricts et API canonique pour autonomie déterministe",
          points: [
            { label: "Gouvernance par l’Écriture", text: "Chaque strate gouverne uniquement en inscrivant ses intentions dans son ledger immuable (Govern by writing)." },
            { label: "Invariants Absolus", text: "Strate 0 n’accède jamais au code applicatif. Strate 2 n’écoute sur aucun port (polling HTTP unilatéral avec digest SHA-256 scellé)." },
            { label: "Auto-Guérison Déterministe", text: "Interface OpenAPI 3.1 permettant à l’agent IA d’inspecter les métriques et de relocaliser les fractales sans aucune intervention humaine." }
          ]
        }
      },
      tag: 'Cœur du Système',
      title: 'PodMesh : Architecture Multi-Fractales & Confinement Souverain',
      lead:
        'PodMesh abandonne le modèle monolithique de Kubernetes au profit d’une fédération fractale décentralisée. Chaque domaine applicatif est un univers étanche gouverné par l’écriture et confiné par userns + cgroups v2.',
      strataTitle: 'Séparation Stricte des 4 Strates (Loi SHAPER OS v1.15)',
      strataDesc:
        'La loi fondamentale impose une séparation stricte des privilèges : le père n’entre jamais dans les entrailles du fils, et le travailleur n’écoute sur aucun port.',
      strata: [
        {
          num: '0',
          name: 'Strate 0 — Univers Père & Manager Souverain',
          actor: 'Manager PodMesh + Agent Pont',
          location: 'Nœuds de contrôle répliqués en Quorum distribué Ed25519',
          power: 'Pouvoir souverain hôte "par en-dessous" (cgroups v2, userns, Btrfs/LVM, migration à chaud).',
          network: 'Réseau d’administration chiffré WireGuard / ZeroTier, Quorum Ed25519.',
          invariant: 'Agit sur le runtime conteneur sans jamais pénétrer la logique métier interne des fractales.',
        },
        {
          num: '1',
          name: 'Strate 1 — Gouverneur de Fractale',
          actor: 'Démon Gouverneur de Domaine (ex: univ-demo-saas-prod)',
          location: 'Univers isolé avec base MariaDB dédiée',
          power: 'Gouverne uniquement par l’écriture (Govern by writing) dans son ledger privé.',
          network: 'N’ouvre JAMAIS de connexion sortante vers les nœuds. Expose 3 endpoints d’autorité.',
          invariant: 'Ne détient aucune clé hôte SSH et ne stocke aucun secret d’infrastructure.',
        },
        {
          num: '2',
          name: 'Strate 2 — Maker Spécialisateur',
          actor: 'Démon ouvrier de nœud d’exécution (podmesh-maker)',
          location: 'Nœud Linux physique ou VM d’exécution',
          power: 'Exécute des recettes scellées et immuables (<hostKind>-<workKind>.sh).',
          network: 'N’écoute sur AUCUN port. Polling HTTP sortant vers son Gouverneur avec digest SHA-256.',
          invariant: 'Zéro travail sans octets prouvés (No work for unproven bytes).',
        },
        {
          num: '3',
          name: 'Strate 3 — Univers Clients Déployés',
          actor: 'Conteneurs et Microservices Spécialisés',
          location: 'Namespaces userns dédiés, volumes persistants Btrfs/MeshFS',
          power: 'Exécution applicative isolée (Boutiques WooCommerce, APIs, SaaS).',
          network: 'Réseau virtuel bridged avec veth dédiée et ingress HTTPS certifié.',
          invariant: 'UID/GID mapping strict (root dans le conteneur = utilisateur non privilégié sur l’hôte).',
        },
      ],
      features: [
        {
          title: 'Isolation userns + cgroups v2',
          desc: 'Confinement matériel absolu. Même une compromission root dans le conteneur ne confère aucun privilège sur le noyau hôte grâce au remapping strict des user namespaces.',
        },
        {
          title: 'API OpenAPI 3.1 Intégrale',
          desc: 'Architecture 100% "API-Sable". Pilotage automatisé par scripts, CLI et agents IA (Claude Code, ChatGPT, Antigravity) avec schémas contractuels stricts.',
        },
        {
          title: 'Active Arbiter & Chaos Automation',
          desc: 'Supervision de quorum multi-sites temps réel avec tests de chaos périodiques automatisés (coupure de liens, kill de nœuds) et bascule transparente en 42ms.',
        },
      ],
    },
    pmbs: {
      perspectives: {
        human: {
          title: "Vision Humain & Décideur (En clair)",
          subtitle: "Une assurance-vie pour vos données avec preuve de survie en temps réel",
          points: [
            { label: "Le Problème Résolu", text: "La plupart des entreprises découvrent que leurs sauvegardes sont illisibles le jour où elles subissent un ransomware ou un sinistre." },
            { label: "La Métaphore Simple", text: "Un exercice d’incendie exécuté après chaque sauvegarde : PMBS redémarre virtuellement vos données en 1 seconde dans un sas d’essai pour vous prouver qu’elles fonctionnent." },
            { label: "Le Gain Direct", text: "Garantie mathématique de redémarrage, restauration chirurgicale d’un seul fichier en 1 clic sans télécharger des téraoctets entiers." }
          ]
        },
        geek: {
          title: "Sous le Capot (Spécifications Geek & Noyau Linux)",
          subtitle: "Déduplication SHA-256 CAS, Op 10 Sandbox Boot et Time-Machine CoW",
          points: [
            { label: "Datastore CAS SHA-256", text: "Déduplication au bloc avec ratio moyen de 4.8x à 12x sur les flottes d’univers hétérogènes." },
            { label: "Preuve de Survie Op 10", text: "Instanciation sandbox éphémère sans réseau en 300ms, audit des bases de données et checks.json en 1.1s, sceau Ed25519." },
            { label: "Time-Travel Forking", text: "Création de branches de staging complètes en 310ms par clonage CoW sans consommer d’espace disque supplémentaire." }
          ]
        },
        agent: {
          title: "Pilotage par Agent IA (Contrats Formels & Machine)",
          subtitle: "Signature cryptographique des snapshots et audit autonome",
          points: [
            { label: "Sceau Numérique Ed25519", text: "Chaque sauvegarde certifiée reçoit une signature immuable. Une sauvegarde non testée par l’Op 10 est rejetée par le système." },
            { label: "Détection d’Anomalie IA", text: "L’agent IA analyse le delta de déduplication : une hausse anormale déclenche une alerte anti-ransomware immédiate." },
            { label: "Restauration Assistée", text: "L’agent IA extrait et réinjecte atomiquement un fichier corrompu en quelques millisecondes via l’API PMBS." }
          ]
        }
      },
      tag: 'Sauvegarde & Continuité',
      title: 'PodMesh Backup Server (PMBS) : L’Immuabilité Certifiée',
      lead:
        'PMBS est un serveur de sauvegarde dédupliqué haute performance, fonctionnant en mode hyperconvergé ou autonome sur un serveur nu sans PodMesh.',
      sha256Title: 'Datastore Dédupliqué SHA-256 Content Addressable Storage (CAS)',
      sha256Desc:
        'Chaque bloc de données est découpé, hashé par SHA-256 et stocké dans un arbre immuable. Les sauvegardes répétées ne consomment que le delta net, atteignant un ratio de déduplication moyen de 4.8x à 12x sur les flottes d’univers.',
      op10Title: 'Preuve de Survie Op 10 — Sandbox Boot Restore',
      op10Desc:
        'Une sauvegarde non testée n’est pas une sauvegarde. PMBS exécute immédiatement l’Opération 10 après chaque snapshot pour certifier l’intégrité.',
      op10Steps: [
        { step: '01', title: 'Snapshot Dédupliqué', text: 'Calcul de l’arbre de Merkle et écriture atomique du delta dans le datastore PMBS.' },
        { step: '02', title: 'Instanciation Sandbox', text: 'Création d’un conteneur éphémère d’isolation sans réseau en ~300ms via clone CoW.' },
        { step: '03', title: 'Boot & Health Check', text: 'Exécution du kernel, montage racine, vérification MariaDB/PostgreSQL et checks.json en 1.1s.' },
        { step: '04', title: 'Sceau Numérique Ed25519', text: 'Signature cryptographique de la preuve de survie inscrite de manière immuable au ledger.' },
      ],
      remoteSyncTitle: 'Remote Sync Multi-Régions : Réplication Inter-Datacenters',
      remoteSyncDesc:
        'Synchronisation chiffrée de datastore à datastore entre sites distants ou régions cloud. En cas de sinistre sur le site primaire, le site secondaire dispose d’une copie qualifiée prête au démarrage immédiat sans perte de données.',
      timeMachineTitle: 'Time-Machine CoW (Copy-on-Write)',
      timeMachineDesc:
        'Explorez l’arborescence historique de vos univers. Restaurez un fichier unitaire (/etc/shaper/status.json) en 1 clic ou forkez une branche de test (Time-Travel Fork) en 310ms sans dupliquer les téraoctets de données.',
    },
    meshfs: {
      perspectives: {
        human: {
          title: "Vision Humain & Décideur (En clair)",
          subtitle: "Un disque dur partagé magique et indestructible pour tous vos serveurs",
          points: [
            { label: "Le Problème Résolu", text: "Comment partager des fichiers et des disques entre tous vos serveurs sans risque de perte et sans installer une usine à gaz ingérable ?" },
            { label: "La Métaphore Simple", text: "Un disque virtuel indestructible : chaque fichier est découpé, dupliqué et synchronisé en temps réel. Si un serveur tombe, vos fichiers restent instantanément accessibles." },
            { label: "Le Gain Direct", text: "Fonctionne dès 1 seul serveur jusqu’à 100+, consomme moins de 64 Mo de mémoire (vs 4 Go par serveur pour Ceph), et s’installe en 1 ligne de commande." }
          ]
        },
        geek: {
          title: "Sous le Capot (Spécifications Geek & Noyau Linux)",
          subtitle: "Stockage CAS, Merkle DAG et exploitation directe de Btrfs/ZFS/LVM",
          points: [
            { label: "Arbre de Merkle DAG", text: "Métadonnées et arborescence indexées sous forme de graphe acyclique dirigé. Réconciliation différentielle instantanée O(1)." },
            { label: "Chunks CAS 4MB", text: "Découpage des flux en blocs CAS (Content Addressable Storage) de 4MB adressés par leur hash cryptographique SHA-256." },
            { label: "Multi-Moteurs Transparent", text: "Exploitation native des subvolumes Btrfs (reflink CoW), ZFS datasets (zstd) ou LVM thin-pools sans couche de traduction superflue." }
          ]
        },
        agent: {
          title: "Pilotage par Agent IA (Contrats Formels & Machine)",
          subtitle: "Vérification mathématique d’intégrité et équilibrage élastique",
          points: [
            { label: "Vérification Formelle", text: "Zéro corruption silencieuse (bit-rot) : chaque chunk est prouvé par son empreinte cryptographique avant toute écriture." },
            { label: "Déterminisme Peer-to-Peer", text: "Topologie dynamique où l’agent IA peut enrôler un nœud (meshfs cluster join) et orchestrer la réplication sans redémarrage." },
            { label: "Interface Machine", text: "Endpoints RPC haute fréquence et CLI modulaire permettant aux agents IA d’assurer l’auto-rééquilibrage du stockage." }
          ]
        }
      },
      tag: 'Stockage Distribué',
      title: 'MeshFS : Système de Fichiers Distribué Élastique',
      lead:
        'Conçu comme l’alternative moderne et légère à CephFS. MeshFS offre la cohérence distribuée et la haute disponibilité sans la monstrueuse complexité opérationnelle des OSD/MON/MDS.',
      merkleTitle: 'Arbre de Merkle Inodes & Chunks CAS 4MB',
      merkleDesc:
        'La hiérarchie des répertoires et métadonnées est stockée sous forme de graphe acyclique dirigé (DAG) de Merkle. Chaque fichier volumineux est segmenté en chunks de 4MB adressés par leur hash.',
      enginesTitle: 'Support Multi-Moteurs Transparent',
      enginesDesc:
        'MeshFS s’appuie sur les briques éprouvées du noyau Linux. Il peut exploiter indifféremment des pools ZFS (zpool), Btrfs (subvolumes CoW), LVM thin-pools ou des répertoires ext4 existants.',
      cephComparisonTitle: 'Comparatif Sans Concession : MeshFS vs CephFS',
      cephComparisonHeaders: ['Critère', 'CephFS', 'MeshFS (PodMesh)', 'Avantage MeshFS'],
      cephComparisonRows: [
        ['Déploiement minimal', '3 à 5 nœuds requis, quorum lourd', '1 nœud autonome jusqu’à N nœuds', 'Élastique dès le 1er serveur'],
        ['Démons requis', 'OSD, MON, MDS, MGR, RGW, CRUSH', '1 seul binaire unifié meshfs-node', 'Zéro complexité opérationnelle'],
        ['Consommation RAM', '2 à 4 GiB par OSD + 4 GiB MDS', '< 64 MiB par nœud', 'Économie x50 sur la mémoire'],
        ['Prise en main & Maintenance', 'Équipe d’ingénieurs stockage dédiée', 'Installable en 1 commande CLI', 'Autonomie totale du devops'],
        ['Snapshots & Diff', 'Snapshots RADOS complexes', 'Arbre de Merkle instantané O(1)', 'Réconciliation différentielle ultra-rapide'],
        ['Indépendance Runtime', 'Couplage fort à la grappe Ceph', 'Fonctionne AVEC ou SANS PodMesh', 'Déployable sur n’importe quel Linux'],
      ],
    },
    proxmox: {
      perspectives: {
        human: {
          title: "Vision Humain & Décideur (En clair)",
          subtitle: "Connectez votre cloud souverain à votre hyperviseur en un clin d’œil",
          points: [
            { label: "Le Problème Résolu", text: "Comment moderniser votre infrastructure de virtualisation sans jeter vos serveurs Proxmox actuels ni changer vos habitudes ?" },
            { label: "La Métaphore Simple", text: "Une prise universelle : MeshFS s’ajoute dans Proxmox comme un simple disque réseau. Vos VMs continuent de tourner, mais avec la réplication et la sauvegarde magique intégrées." },
            { label: "Le Gain Direct", text: "Migration à chaud de VMs entre serveurs en direct, zéro coût de licence propriétaire VMware/SAN." }
          ]
        },
        geek: {
          title: "Sous le Capot (Spécifications Geek & Noyau Linux)",
          subtitle: "Driver Perl PVE::Storage natif, support QEMU Raw/Qcow2 et LXC rootdir",
          points: [
            { label: "Architecture Driver", text: "Extension propre de PVE::Storage::Plugin sans injection risquée dans les binaires système de Proxmox." },
            { label: "Formats Supportés", text: "Images disques VM (.raw, .qcow2), rootfs conteneurs LXC et archives VZDump stockées directement sur MeshFS." },
            { label: "Hooks Proxmox", text: "Gestion transparente de alloc_image, free_image, volume_snapshot, et activate_storage avec déduplication." }
          ]
        },
        agent: {
          title: "Pilotage par Agent IA (Contrats Formels & Machine)",
          subtitle: "Provisioning déclaratif et surveillance continue de la grappe",
          points: [
            { label: "Déclaration Déclarative", text: "Génération automatique et validation syntaxique de /etc/pve/storage.cfg par l’agent IA via pvesm." },
            { label: "Audit de Santé", text: "Interrogation en continu du endpoint /api/v1/meshfs/status pour anticiper les saturations d’espace disque." },
            { label: "Orchestration VM", text: "L’agent IA peut provisionner une VM complète et son stockage répliqué via une commande unifiée." }
          ]
        }
      },
      tag: 'Virtualisation Native',
      title: 'Plugin de Stockage Officiel pour Proxmox VE',
      lead:
        'Intégrez PodMesh et MeshFS directement dans l’interface web de Proxmox Virtual Environment (PVE 8.x & 9.x) grâce au plugin de stockage officiel.',
      features: [
        {
          title: 'Support Complet VM QEMU',
          desc: 'Allocation et exécution d’images disques VM (.raw, .qcow2) stockées directement sur les volumes répliqués MeshFS avec performances natives I/O.',
        },
        {
          title: 'Rootfs Conteneurs LXC',
          desc: 'Instanciez des conteneurs LXC légers avec leur rootdir monté sur des subvolumes dédupliqués et protégés par snapshots instantanés.',
        },
        {
          title: 'Cible de Sauvegarde VZDump',
          desc: 'Proxmox sauvegarde nativement vos VMs et CTs directement vers le datastore PMBS avec déduplication SHA-256 et vérification de boot.',
        },
      ],
      quickInstallTitle: 'Installation en 1-Clic sur Proxmox VE',
      quickInstallDesc: 'Exécutez cette commande unique sur votre nœud Proxmox en root :',
    },
    downloads: {
      tag: 'Paquets & CLI',
      title: 'Téléchargements Officiels & Cheatsheet CLI',
      lead:
        'Tous les paquets Debian (.deb) sont signés cryptographiquement et maintenus pour Debian 12 (Bookworm) et Ubuntu 24.04 LTS.',
      packagesTitle: 'Paquets Debian (.deb) Disponibles',
      packages: [
        {
          name: 'podmesh-core',
          version: '2.4.0',
          arch: 'amd64',
          desc: 'Runtime moteur PodMesh, gouverneur de strate 0 et agent pont.',
          file: 'podmesh-core_2.4.0_amd64.deb',
          size: '14.2 MB',
        },
        {
          name: 'pmbs-server',
          version: '2.4.0',
          arch: 'amd64',
          desc: 'Serveur autonome de sauvegarde dédupliquée et exécuteur Op 10.',
          file: 'pmbs-server_2.4.0_amd64.deb',
          size: '9.8 MB',
        },
        {
          name: 'meshfs-node',
          version: '2.4.0',
          arch: 'amd64',
          desc: 'Nœud de système de fichiers distribué léger avec arbre de Merkle.',
          file: 'meshfs-node_2.4.0_amd64.deb',
          size: '8.4 MB',
        },
        {
          name: 'pve-storage-podmesh',
          version: '2.4.0',
          arch: 'all',
          desc: 'Plugin de stockage Perl pour Proxmox VE (QEMU, LXC, VZDump).',
          file: 'pve-storage-podmesh_2.4.0_all.deb',
          size: '1.2 MB',
        },
      ],
      cliTitle: 'Cheatsheet CLI Interactive',
      cliTabs: {
        podmesh: 'podmesh',
        pmbs: 'pmbs-cli',
        meshfs: 'meshfs',
      },
    },
  },
  en: {
    github: {
      title: "Official GitHub Repositories",
      podmeshUrl: "https://github.com/xavdp-pro/podmesh",
      shaperUrl: "https://github.com/xavdp-pro/SHAPER-OS-V1.13",
      podmeshLabel: "PodMesh GitHub Repository",
      shaperLabel: "Shaper OS GitHub Repository",
    },
    tandem: {
      badge: "Shaper OS Doctrine • Human ⇄ AI Agent Tandem",
      title: "Natively Designed for the Symbiotic Human & AI Agent Tandem",
      lead: "PodMesh and Shaper OS are neither rigid manual tools nor uncontrollable black boxes. The architecture is engineered from the ground up for a co-pilot relationship where humans provide strategic intent and AI agents ensure mathematical rigor and real-time self-healing.",
      levels: [
        {
          id: "human",
          name: "Human & Decision Maker (Non-Geek)",
          role: "Strategic Intent & Sovereign Ownership",
          summary: "Clear understanding without technical jargon: slashed infrastructure bills, zero vendor lock-in, bulletproof resilience.",
          badge: "Business Vision"
        },
        {
          id: "geek",
          name: "Systems Engineer (Geek)",
          role: "Linux Kernel, CAS & Protocols",
          summary: "Low-level system architecture: Merkle DAG, 4MB CAS chunks, userns UID remapping, cgroups v2, Ed25519 quorum.",
          badge: "System Specs"
        },
        {
          id: "agent",
          name: "Autonomous AI Agent (Machine)",
          role: "Formal Invariants & Deterministic API",
          summary: "Rigorous machine contracts: zero ambiguity, state verified by SHA-256 digests, govern-by-writing, and self-healing.",
          badge: "Machine Contracts"
        }
      ]
    },
    hero: {
      badge: 'Official Architecture • SHAPER OS v1.15',
      title: 'PodMesh Cloud OS & Multi-Fractal Federation',
      subtitle:
        'Sovereign orchestration platform, distributed MeshFS storage, and immutable PMBS backup for mission-critical infrastructure and Proxmox VE.',
      btnMeshfs: 'MeshFS Specifications',
      btnQuickstart: 'CLI Quickstart',
      btnExplore: 'Explore Architecture',
    },
    metrics: [
      { label: 'Consensus & Quorum', value: 'Zero Split-Brain', sub: 'Ed25519 Cryptographic Signatures' },
      { label: 'High Availability', value: '< 50 ms', sub: 'Btrfs & ZFS CoW Failover' },
      { label: 'Survival Proof Op 10', value: '100% Verified', sub: 'Sandbox Boot Restore Validation' },
      { label: 'MeshFS RAM Footprint', value: '< 64 MiB', sub: 'vs 4 GiB+ per Ceph OSD' },
    ],
    navTabs: {
      podmesh: 'PodMesh Engine',
      pmbs: 'PMBS Backup Server',
      meshfs: 'Distributed MeshFS',
      proxmox: 'Proxmox VE Plugin',
      downloads: 'Downloads & CLI',
    },
    podmesh: {
      perspectives: {
        human: {
          title: "Human & Decision Maker View (Clear Language)",
          subtitle: "Why PodMesh frees your business from big-tech cloud dependence",
          points: [
            { label: "The Problem Solved", text: "How to run mission-critical apps on your own servers without paying exorbitant AWS bills or hiring an army of cloud engineers?" },
            { label: "Simple Metaphor", text: "An invisible conductor: if a physical server catches fire, your workloads migrate to another host in 42ms with zero downtime for users." },
            { label: "Direct Business Value", text: "10x lower infrastructure cost, sovereign data ownership, and complete immunity to vendor lock-in." }
          ]
        },
        geek: {
          title: "Under The Hood (Linux Kernel & Systems Geek Specs)",
          subtitle: "Fractal orchestration without monolithic etcd and hermetic userns isolation",
          points: [
            { label: "Architecture", text: "Strict 4-strata separation (Shaper OS v1.15 Law). The parent never enters the internal runtime of child fractales." },
            { label: "Kernel Confinement", text: "userns UID remapping (100000..165535) and cgroups v2 controllers (cpu, memory, io, pids). Root in container = unprivileged on host." },
            { label: "High Availability", text: "Fault-tolerant (2F+1) Ed25519 distributed quorum with epoch consensus and < 50ms live migration via Btrfs/ZFS reflink snapshots." }
          ]
        },
        agent: {
          title: "Autonomous AI Agent Control (Formal Machine Contracts)",
          subtitle: "Strict invariants and canonical API for deterministic self-healing",
          points: [
            { label: "Govern by Writing", text: "Each stratum governs exclusively by appending declarative intents into its private immutable ledger." },
            { label: "Absolute Invariants", text: "Stratum 0 never touches application code. Stratum 2 listens on NO port (outbound polling with SHA-256 sealed digests)." },
            { label: "Autonomous Self-Healing", text: "OpenAPI 3.1 interface enabling AI agents to inspect metrics and reallocate workloads with zero human intervention." }
          ]
        }
      },      tag: 'System Core',
      title: 'PodMesh: Multi-Fractal Architecture & Sovereign Confinement',
      lead:
        'PodMesh rejects the monolithic complexity of Kubernetes in favor of a decentralized fractal federation. Each application domain is an airtight universe governed by writing and confined via userns + cgroups v2.',
      strataTitle: 'Strict Separation of the 4 Strata (SHAPER OS Law v1.15)',
      strataDesc:
        'Fundamental law mandates strict separation of powers: the parent never enters the child internals, and the worker daemon listens on no network ports.',
      strata: [
        {
          num: '0',
          name: 'Stratum 0 — Parent Universe & Sovereign Manager',
          actor: 'PodMesh Manager + Bridge Agent',
          location: 'Control plane nodes replicated across distributed Ed25519 Quorum',
          power: 'Sovereign host authority from underneath (cgroups v2, userns, Btrfs/LVM, live migration).',
          network: 'Encrypted WireGuard / ZeroTier control plane, Ed25519 Quorum.',
          invariant: 'Acts on container runtimes without ever intruding into internal domain business logic.',
        },
        {
          num: '1',
          name: 'Stratum 1 — Fractal Governor',
          actor: 'Domain Governor Daemon (e.g. univ-demo-saas-prod)',
          location: 'Isolated universe with private MariaDB ledger',
          power: 'Governs strictly by writing into its private ledger.',
          network: 'NEVER opens outgoing connections to cluster nodes. Exposes 3 authoritative endpoints.',
          invariant: 'Holds no host SSH keys and stores no infrastructure root secrets.',
        },
        {
          num: '2',
          name: 'Stratum 2 — Specializer Maker',
          actor: 'Node Worker Daemon (podmesh-maker)',
          location: 'Physical Linux node or execution VM',
          power: 'Executes sealed immutable recipes (<hostKind>-<workKind>.sh).',
          network: 'Listens on NO network ports. Polls Governor outbound with SHA-256 inventory digests.',
          invariant: 'Zero work for unproven bytes (No work for unproven bytes).',
        },
        {
          num: '3',
          name: 'Stratum 3 — Deployed Client Universes',
          actor: 'Specialized Containers & Microservices',
          location: 'Dedicated userns namespaces, persistent Btrfs/MeshFS volumes',
          power: 'Airtight application workload execution (WooCommerce shops, APIs, SaaS apps).',
          network: 'Bridged virtual network with dedicated veth pair and TLS ingress.',
          invariant: 'Strict UID/GID mapping (root in container = unprivileged user on host).',
        },
      ],
      features: [
        {
          title: 'userns + cgroups v2 Confinement',
          desc: 'Absolute hardware boundary. Even a full root compromise inside a container cannot break out to the host kernel due to strict non-overlapping user namespaces.',
        },
        {
          title: 'Comprehensive OpenAPI 3.1',
          desc: '100% "API-Able" architecture. Complete programmable control for automation, CLI utilities, and autonomous AI agents (Claude Code, ChatGPT, Antigravity).',
        },
        {
          title: 'Active Arbiter & Chaos Automation',
          desc: 'Real-time multi-site quorum monitoring with recurring automated chaos engineering experiments and seamless 42ms failover.',
        },
      ],
    },
    pmbs: {
      tag: 'Backup & Disaster Recovery',
      title: 'PodMesh Backup Server (PMBS): Certified Immutability',
      lead:
        'PMBS is a high-performance deduplicated backup server operating either hyperconverged or as a standalone baremetal server without PodMesh installed.',
      sha256Title: 'Deduplicated SHA-256 Content Addressable Storage (CAS)',
      sha256Desc:
        'Data blocks are sliced, SHA-256 hashed, and stored in an immutable tree. Repeated backups only store net deltas, delivering typical 4.8x to 12x deduplication ratios.',
      op10Title: 'Op 10 Survival Seal — Sandbox Boot Restore',
      op10Desc:
        'An untested backup is not a backup. PMBS automatically executes Operation 10 immediately following every snapshot to prove restore viability.',
      op10Steps: [
        { step: '01', title: 'Deduplicated Snapshot', text: 'Merkle tree computation and atomic delta ingestion into the PMBS datastore.' },
        { step: '02', title: 'Sandbox Instantiation', text: 'Ephemeral isolated container spawned in ~300ms using instant CoW cloning without network.' },
        { step: '03', title: 'Boot & Health Check', text: 'Kernel execution, root mount, database integrity validation and checks.json audit in 1.1s.' },
        { step: '04', title: 'Ed25519 Survival Seal', text: 'Cryptographic signature of the survival proof permanently sealed into the ledger.' },
      ],
      remoteSyncTitle: 'Multi-Region Remote Sync: Inter-Datacenter Replication',
      remoteSyncDesc:
        'Encrypted datastore-to-datastore replication across remote facilities or cloud regions. Complete primary site failure yields zero data loss with instant standby recovery.',
      timeMachineTitle: 'Time-Machine CoW (Copy-on-Write)',
      timeMachineDesc:
        'Browse historical snapshots down to individual files (/etc/shaper/status.json) or fork a live testing universe (Time-Travel Fork) in 310ms without disk duplication.',
    },
    meshfs: {
      perspectives: {
        human: {
          title: "Human & Decision Maker View (Clear Language)",
          subtitle: "A magic, indestructible distributed shared disk for all your servers",
          points: [
            { label: "The Problem Solved", text: "How to share files and virtual disks across servers without risking data loss and without installing bloated complex clusters?" },
            { label: "Simple Metaphor", text: "An unbreakable virtual hard drive: every file is sliced, replicated, and synced in real-time. If a server dies, all files remain online." },
            { label: "Direct Business Value", text: "Operates from 1 node up to hundreds, uses < 64MB RAM (vs 4GB+ per Ceph OSD), installs in a single command." }
          ]
        },
        geek: {
          title: "Under The Hood (Linux Kernel & Systems Geek Specs)",
          subtitle: "CAS storage, Merkle DAG and direct native Btrfs/ZFS/LVM acceleration",
          points: [
            { label: "Merkle DAG Tree", text: "Metadata and directory hierarchies structured as a directed acyclic graph. O(1) instantaneous differential reconciliation." },
            { label: "4MB CAS Chunks", text: "I/O stream chunking into 4MB immutable Content Addressable Storage blocks addressed by SHA-256 hashes." },
            { label: "Transparent Multi-Engines", text: "Native integration with Btrfs subvolumes (reflink CoW), ZFS datasets (zstd compression), or LVM thin-pools." }
          ]
        },
        agent: {
          title: "Autonomous AI Agent Control (Formal Machine Contracts)",
          subtitle: "Mathematical integrity verification and elastic peer-to-peer balancing",
          points: [
            { label: "Formal Proof", text: "Zero silent data corruption (bit-rot): every chunk is cryptographically validated before commit." },
            { label: "Deterministic P2P", text: "Dynamic topology allowing AI agents to join nodes (meshfs cluster join) and rebalance without service restarts." },
            { label: "Machine Interface", text: "High-frequency RPC endpoints and modular CLI enabling autonomous AI storage management." }
          ]
        }
      },      tag: 'Distributed Storage',
      title: 'MeshFS: Lightweight Distributed File System',
      lead:
        'Engineered as the modern, zero-overhead alternative to CephFS. MeshFS delivers distributed consistency and high availability without the crushing operational complexity of Ceph.',
      merkleTitle: 'Merkle Tree Inodes & 4MB CAS Chunks',
      merkleDesc:
        'Directory hierarchies and metadata form a Merkle Directed Acyclic Graph (DAG). Large files and virtual disk images are chunked into 4MB blocks addressed by content hash.',
      enginesTitle: 'Transparent Multi-Engine Backend',
      enginesDesc:
        'MeshFS leverages proven Linux kernel foundations, interchangeably supporting ZFS zpools, Btrfs CoW subvolumes, LVM thin pools, or standard ext4 folders.',
      cephComparisonTitle: 'Uncompromising Benchmark: MeshFS vs CephFS',
      cephComparisonHeaders: ['Criterion', 'CephFS', 'MeshFS (PodMesh)', 'MeshFS Advantage'],
      cephComparisonRows: [
        ['Minimum Deployment', '3 to 5 nodes required, complex quorum', '1 standalone node up to N nodes', 'Elastic from server #1'],
        ['Required Daemons', 'OSD, MON, MDS, MGR, RGW, CRUSH map', 'Single unified meshfs-node binary', 'Zero operational overhead'],
        ['RAM Consumption', '2 to 4 GiB per OSD + 4 GiB MDS', '< 64 MiB per cluster node', '50x memory savings'],
        ['Operations & Maintenance', 'Requires dedicated storage team', 'Installed with 1 simple CLI command', 'Full DevOps autonomy'],
        ['Snapshots & Diffing', 'Complex RADOS snapshot machinery', 'Instant O(1) Merkle Tree snapshots', 'Blazing-fast differential sync'],
        ['Runtime Coupling', 'Tightly bound to Ceph cluster', 'Runs WITH or WITHOUT PodMesh', 'Deployable on any Linux box'],
      ],
    },
    proxmox: {
      perspectives: {
        human: {
          title: "Human & Executive View (Plain Language)",
          subtitle: "Connect your sovereign cloud to your hypervisor in seconds",
          points: [
            { label: "Problem Solved", text: "How to modernize your virtualization without discarding existing Proxmox servers or changing operational habits?" },
            { label: "Simple Metaphor", text: "A universal adapter: MeshFS attaches to Proxmox just like a standard network drive, bringing transparent replication and instant backups to your existing VMs." },
            { label: "Immediate Benefit", text: "Seamless live VM migration between nodes, zero proprietary VMware or SAN licensing fees." },
          ],
        },
        geek: {
          title: "Under The Hood (Geek & Linux Kernel Details)",
          subtitle: "Native PVE::Storage Perl plugin, QEMU Raw/Qcow2 & LXC rootdir support",
          points: [
            { label: "Driver Architecture", text: "Clean extension of PVE::Storage::Plugin without monkey-patching or modifying Proxmox core binaries." },
            { label: "Supported Formats", text: "VM virtual disks (.raw, .qcow2), LXC container root directories, and VZDump backup archives stored on MeshFS." },
            { label: "Proxmox Hooks", text: "Transparent implementation of alloc_image, free_image, volume_snapshot, and activate_storage with block deduplication." },
          ],
        },
        agent: {
          title: "AI Agent Orchestration (Formal Contracts & Machine)",
          subtitle: "Declarative storage provisioning and real-time cluster health monitoring",
          points: [
            { label: "Declarative Ledger", text: "Automated generation and syntactic validation of /etc/pve/storage.cfg by AI agents via pvesm CLI." },
            { label: "Health Audits", text: "Continuous polling of /api/v1/meshfs/status to forecast capacity thresholds and prevent disk exhaustion." },
            { label: "VM Orchestration", text: "The AI agent can provision full VM instances along with replicated MeshFS storage in a single automated step." },
          ],
        },
      },
      tag: 'Native Virtualization',
      title: 'Official Proxmox VE Storage Plugin',
      lead:
        'Integrate PodMesh and MeshFS directly into the Proxmox Virtual Environment (PVE 8.x & 9.x) web UI via our official Perl storage backend plugin.',
      features: [
        {
          title: 'Full QEMU VM Support',
          desc: 'Allocate and run virtual machine disk images (.raw, .qcow2) stored directly on replicated MeshFS volumes with native near-metal I/O performance.',
        },
        {
          title: 'LXC Container Rootfs',
          desc: 'Instantiate ultra-lightweight LXC containers with root filesystems hosted on deduplicated subvolumes protected by instant snapshots.',
        },
        {
          title: 'Direct VZDump Target',
          desc: 'Proxmox backs up VMs and containers directly to the PMBS datastore with block-level SHA-256 deduplication and sandbox boot verification.',
        },
      ],
      quickInstallTitle: '1-Click Install on Proxmox VE',
      quickInstallDesc: 'Run this single command as root on your Proxmox VE host:',
    },
    downloads: {
      tag: 'Packages & CLI',
      title: 'Official Downloads & CLI Cheatsheet',
      lead:
        'All Debian packages (.deb) are cryptographically signed and officially maintained for Debian 12 (Bookworm) and Ubuntu 24.04 LTS.',
      packagesTitle: 'Available Debian (.deb) Packages',
      packages: [
        {
          name: 'podmesh-core',
          version: '2.4.0',
          arch: 'amd64',
          desc: 'PodMesh runtime engine, stratum 0 governor and host bridge agent.',
          file: 'podmesh-core_2.4.0_amd64.deb',
          size: '14.2 MB',
        },
        {
          name: 'pmbs-server',
          version: '2.4.0',
          arch: 'amd64',
          desc: 'Standalone deduplicated backup server and Op 10 sandbox execution runner.',
          file: 'pmbs-server_2.4.0_amd64.deb',
          size: '9.8 MB',
        },
        {
          name: 'meshfs-node',
          version: '2.4.0',
          arch: 'amd64',
          desc: 'Lightweight distributed file system node with Merkle tree inodes.',
          file: 'meshfs-node_2.4.0_amd64.deb',
          size: '8.4 MB',
        },
        {
          name: 'pve-storage-podmesh',
          version: '2.4.0',
          arch: 'all',
          desc: 'Perl storage plugin for Proxmox VE (QEMU, LXC, VZDump).',
          file: 'pve-storage-podmesh_2.4.0_all.deb',
          size: '1.2 MB',
        },
      ],
      cliTitle: 'Interactive CLI Cheatsheet',
      cliTabs: {
        podmesh: 'podmesh',
        pmbs: 'pmbs-cli',
        meshfs: 'meshfs',
      },
    },
  },
  es: {
    github: {
      title: "Repositorios Oficiales en GitHub",
      podmeshUrl: "https://github.com/xavdp-pro/podmesh",
      shaperUrl: "https://github.com/xavdp-pro/SHAPER-OS-V1.13",
      podmeshLabel: "Repositorio GitHub PodMesh",
      shaperLabel: "Repositorio GitHub Shaper OS",
    },
    tandem: {
      badge: "Doctrina Shaper OS • Tándem Humano ⇄ Agente IA",
      title: "Diseñado Nativamente para el Tándem Simbiótico Humano y Agente IA",
      lead: "PodMesh y Shaper OS no son herramientas manuales rígidas ni cajas negras incontrolables. La arquitectura está forjada para un co-pilotaje donde el humano establece la intención estratégica y el agente IA garantiza el rigor matemático y la autorreparación.",
      levels: [
        {
          id: "human",
          name: "Humano y Tomador de Decisiones (No-Geek)",
          role: "Intención y Gobernanza Soberana",
          summary: "Comprensión clara sin tecnicismos: reducción drástica de costes de nube, cero dependencia de proveedores, resiliencia total.",
          badge: "Visión de Negocio"
        },
        {
          id: "geek",
          name: "Ingeniero de Sistemas (Geek)",
          role: "Núcleo Linux, CAS y Protocolos",
          summary: "Bajo nivel: Árbol Merkle DAG, chunks CAS 4MB, mapeo UID userns, cgroups v2, quórum criptográfico Ed25519.",
          badge: "Especificaciones Técnicas"
        },
        {
          id: "agent",
          name: "Agente IA Autónomo (Máquina)",
          role: "Invariantes Formales y API Determinista",
          summary: "Reglas exactas para la IA: cero ambigüedad, validación por hashes SHA-256, gobernanza por escritura y autorreparación.",
          badge: "Contratos de Máquina"
        }
      ]
    },
    hero: {
      badge: 'Arquitectura Oficial • SHAPER OS v1.15',
      title: 'PodMesh Cloud OS y Federación Multifractal',
      subtitle:
        'Plataforma de orquestación soberana, almacenamiento distribuido MeshFS y respaldo inmutable PMBS para infraestructuras críticas y Proxmox VE.',
      btnMeshfs: 'Especificaciones MeshFS',
      btnQuickstart: 'Guía Rápida CLI',
      btnExplore: 'Explorar Arquitectura',
    },
    metrics: [
      { label: 'Consenso y Quórum', value: 'Cero Split-Brain', sub: 'Firmas criptográficas Ed25519' },
      { label: 'Alta Disponibilidad', value: '< 50 ms', sub: 'Conmutación CoW Btrfs y ZFS' },
      { label: 'Prueba de Supervivencia', value: '100% Calificada', sub: 'Validación Sandbox Boot Restore' },
      { label: 'Huella RAM MeshFS', value: '< 64 MiB', sub: 'vs 4 GiB+ por OSD Ceph' },
    ],
    navTabs: {
      podmesh: 'Motor PodMesh',
      pmbs: 'PMBS Backup Server',
      meshfs: 'MeshFS Distribuido',
      proxmox: 'Plugin Proxmox VE',
      downloads: 'Descargas y CLI',
    },
    podmesh: {
      tag: 'Núcleo del Sistema',
      title: 'PodMesh: Arquitectura Multifractal y Confinamiento Soberano',
      lead:
        'PodMesh descarta el modelo monolítico de Kubernetes en favor de una federación fractal descentralizada. Cada dominio aplicativo es un universo estanco gobernado por escritura y confinado mediante userns + cgroups v2.',
      strataTitle: 'Separación Estricta de los 4 Estratos (Ley SHAPER OS v1.15)',
      strataDesc:
        'La ley fundamental impone una separación estricta de poderes: el padre nunca penetra en las entrañas del hijo y el ejecutor no escucha en ningún puerto de red.',
      strata: [
        {
          num: '0',
          name: 'Estrato 0 — Universo Padre y Administrador Soberano',
          actor: 'Manager PodMesh + Agente Puente',
          location: 'Nodos de control replicados en Quórum distribuido Ed25519',
          power: 'Poder soberano de host "desde abajo" (cgroups v2, userns, Btrfs/LVM, migración en caliente).',
          network: 'Red de administración cifrada WireGuard / ZeroTier, Quórum Ed25519.',
          invariant: 'Actúa sobre el entorno de contenedores sin invadir la lógica de negocio interna.',
        },
        {
          num: '1',
          name: 'Estrato 1 — Gobernador de Fractal',
          actor: 'Demonio Gobernador de Dominio (ej: univ-demo-saas-prod)',
          location: 'Universo aislado con base de datos MariaDB dedicada',
          power: 'Gobierna estrictamente mediante escritura (Govern by writing) en su ledger privado.',
          network: 'NUNCA abre conexiones salientes hacia los nodos. Expone 3 rutas de autoridad.',
          invariant: 'No retiene claves SSH del host ni almacena credenciales de infraestructura.',
        },
        {
          num: '2',
          name: 'Estrato 2 — Maker Especializador',
          actor: 'Demonio obrero del nodo de ejecución (podmesh-maker)',
          location: 'Nodo físico Linux o máquina virtual de ejecución',
          power: 'Ejecuta recetas selladas e inmutables (<hostKind>-<workKind>.sh).',
          network: 'No escucha en NINGÚN puerto. Sondeo HTTP saliente hacia su Gobernador con hash SHA-256.',
          invariant: 'Sin trabajo para bytes no verificados (No work for unproven bytes).',
        },
        {
          num: '3',
          name: 'Estrato 3 — Universos Clientes Desplegados',
          actor: 'Contenedores y Microservicios Especializados',
          location: 'Namespaces userns dedicados, volúmenes persistentes Btrfs/MeshFS',
          power: 'Ejecución de cargas de trabajo aisladas (Tiendas WooCommerce, APIs, SaaS).',
          network: 'Red virtual en puente con interfaz veth dedicada y terminación TLS.',
          invariant: 'Mapeo UID/GID estricto (root en el contenedor = usuario sin privilegios en el host).',
        },
      ],
      features: [
        {
          title: 'Aislamiento userns + cgroups v2',
          desc: 'Frontera de hardware infranqueable. Ni siquiera un compromiso root dentro del contenedor puede escalar privilegios hacia el kernel anfitrión.',
        },
        {
          title: 'API OpenAPI 3.1 Completa',
          desc: 'Arquitectura 100% "API-Able". Control automatizado por scripts, CLI y agentes de IA autónomos (Claude Code, ChatGPT, Antigravity).',
        },
        {
          title: 'Active Arbiter y Automatización de Caos',
          desc: 'Monitoreo de quórum multisitio en tiempo real con pruebas de caos automáticas y conmutación por error en 42 ms sin interrupciones.',
        },
      ],
    },
    pmbs: {
      tag: 'Copia de Seguridad y Resiliencia',
      title: 'PodMesh Backup Server (PMBS): Inmutabilidad Certificada',
      lead:
        'PMBS es un servidor de respaldo desduplicado de alto rendimiento, capaz de operar de manera hiperconvergente o en un servidor dedicado sin PodMesh instalado.',
      sha256Title: 'Datastore Desduplicado SHA-256 Content Addressable Storage (CAS)',
      sha256Desc:
        'Los bloques de datos se dividen, se procesan con SHA-256 y se guardan en un árbol inmutable. Las copias periódicas solo almacenan el delta real, alcanzando ratios de 4.8x a 12x.',
      op10Title: 'Prueba de Supervivencia Op 10 — Sandbox Boot Restore',
      op10Desc:
        'Una copia de seguridad no verificada no es una copia. PMBS ejecuta automáticamente la Operación 10 tras cada instantánea para certificar su recuperabilidad.',
      op10Steps: [
        { step: '01', title: 'Instantánea Desduplicada', text: 'Cálculo del árbol de Merkle y escritura atómica del delta en el datastore PMBS.' },
        { step: '02', title: 'Instanciación Sandbox', text: 'Creación de un contenedor efímero aislado sin red en ~300ms mediante clon CoW.' },
        { step: '03', title: 'Boot y Verificación', text: 'Arranque del kernel, montaje raíz, verificación de base de datos y checks.json en 1.1s.' },
        { step: '04', title: 'Sello Digital Ed25519', text: 'Firma criptográfica de la prueba de supervivencia registrada de forma inmutable en el ledger.' },
      ],
      remoteSyncTitle: 'Sincronización Remota Multirregión: Replicación Inter-Datacenter',
      remoteSyncDesc:
        'Sincronización cifrada de datastore a datastore entre sitios remotos o regiones cloud. La pérdida total del sitio primario permite la recuperación inmediata en el sitio de respaldo sin pérdida de datos.',
      timeMachineTitle: 'Time-Machine CoW (Copy-on-Write)',
      timeMachineDesc:
        'Explore instantáneas históricas a nivel de archivo individual (/etc/shaper/status.json) o cree ramas de prueba instantáneas (Time-Travel Fork) en 310 ms sin duplicar espacio.',
    },
    meshfs: {
      tag: 'Almacenamiento Distribuido',
      title: 'MeshFS: Sistema de Archivos Distribuido Ultraligero',
      lead:
        'Diseñado como la alternativa moderna y eficiente a CephFS. MeshFS brinda consistencia distribuida y alta disponibilidad sin la abrumadora complejidad operativa de Ceph.',
      merkleTitle: 'Árbol de Merkle Inodes y Chunks CAS de 4MB',
      merkleDesc:
        'La estructura jerárquica y los metadatos se almacenan en un grafo acíclico dirigido (DAG) de Merkle. Los archivos grandes e imágenes se dividen en chunks de 4MB direccionados por su hash.',
      enginesTitle: 'Compatibilidad Multimotor Transparente',
      enginesDesc:
        'MeshFS opera sobre tecnologías contrastadas del kernel Linux: pools ZFS (zpool), subvolúmenes CoW de Btrfs, thin-pools LVM o directorios ext4 estándar.',
      cephComparisonTitle: 'Comparativa Rigurosa: MeshFS vs CephFS',
      cephComparisonHeaders: ['Criterio', 'CephFS', 'MeshFS (PodMesh)', 'Ventaja MeshFS'],
      cephComparisonRows: [
        ['Despliegue mínimo', '3 a 5 nodos obligatorios, quórum pesado', '1 nodo autónomo hasta N nodos', 'Elástico desde el primer servidor'],
        ['Demonios requeridos', 'OSD, MON, MDS, MGR, RGW, CRUSH map', 'Un único binario unificado meshfs-node', 'Cero complejidad operativa'],
        ['Consumo de RAM', '2 a 4 GiB por OSD + 4 GiB MDS', '< 64 MiB por nodo', 'Ahorro x50 en memoria RAM'],
        ['Mantenimiento y Operación', 'Requiere equipo especializado de storage', 'Se instala con un único comando CLI', 'Autonomía total para DevOps'],
        ['Instantáneas y Diferenciales', 'Mecanismo RADOS complejo', 'Instantáneas en árbol de Merkle O(1)', 'Sincronización diferencial ultrarrápida'],
        ['Independencia del Runtime', 'Acoplamiento estricto al clúster Ceph', 'Funciona CON o SIN PodMesh', 'Desplegable en cualquier Linux'],
      ],
    },
    proxmox: {
      perspectives: {
        human: {
          title: "Visión Humana y Directiva (En lenguaje claro)",
          subtitle: "Conecte su nube soberana a su hipervisor en segundos",
          points: [
            { label: "Problema Resuelto", text: "¿Cómo modernizar su infraestructura de virtualización sin descartar sus servidores Proxmox existentes ni cambiar sus hábitos?" },
            { label: "Metáfora Simple", text: "Un enchufe universal: MeshFS se añade en Proxmox como un disco de red estándar, aportando replicación transparente y copias de seguridad mágicas a sus VMs." },
            { label: "Beneficio Directo", text: "Migración en caliente de VMs entre servidores sin interrupción y sin costes de licencia propietarios." },
          ],
        },
        geek: {
          title: "Bajo el Capó (Especificaciones Geek y Kernel Linux)",
          subtitle: "Controlador Perl PVE::Storage nativo, soporte QEMU Raw/Qcow2 y rootdir LXC",
          points: [
            { label: "Arquitectura del Driver", text: "Extensión limpia de PVE::Storage::Plugin sin alterar binarios del sistema de Proxmox." },
            { label: "Formatos Soportados", text: "Imágenes de disco VM (.raw, .qcow2), directorios raíz LXC y archivos VZDump almacenados en MeshFS." },
            { label: "Hooks de Proxmox", text: "Gestión transparente de alloc_image, free_image, volume_snapshot y activate_storage con desduplicación." },
          ],
        },
        agent: {
          title: "Control por Agente IA (Contratos Formales y Máquina)",
          subtitle: "Aprovisionamiento declarativo y supervisión continua del clúster",
          points: [
            { label: "Configuración Declarativa", text: "Generación automática y validación de /etc/pve/storage.cfg por el agente IA mediante pvesm." },
            { label: "Auditoría de Salud", text: "Consulta continua del endpoint /api/v1/meshfs/status para anticipar saturaciones de disco." },
            { label: "Orquestación de VMs", text: "El agente IA puede aprovisionar una máquina virtual completa con almacenamiento replicado en una sola orden." },
          ],
        },
      },
      tag: 'Virtualización Nativa',
      title: 'Plugin Oficial de Almacenamiento para Proxmox VE',
      lead:
        'Integre PodMesh y MeshFS directamente en la consola web de Proxmox Virtual Environment (PVE 8.x y 9.x) mediante nuestro plugin nativo en Perl.',
      features: [
        {
          title: 'Soporte Total Máquinas Virtuales QEMU',
          desc: 'Asigne y ejecute imágenes de disco VM (.raw, .qcow2) alojadas directamente en volúmenes MeshFS con rendimiento I/O casi nativo.',
        },
        {
          title: 'Rootfs Contenedores LXC',
          desc: 'Instancie contenedores LXC con su sistema de archivos raíz montado en subvolúmenes desduplicados y respaldados por snapshots instantáneos.',
        },
        {
          title: 'Destino Nativo para VZDump',
          desc: 'Proxmox guarda copias de seguridad de VMs y CTs directamente en el datastore PMBS con desduplicación SHA-256 y verificación de arranque.',
        },
      ],
      quickInstallTitle: 'Instalación en 1-Clic en Proxmox VE',
      quickInstallDesc: 'Ejecute este comando como root en su nodo Proxmox VE:',
    },
    downloads: {
      tag: 'Paquetes y CLI',
      title: 'Descargas Oficiales y Cheatsheet CLI',
      lead:
        'Todos los paquetes Debian (.deb) están firmados criptográficamente y certificados para Debian 12 (Bookworm) y Ubuntu 24.04 LTS.',
      packagesTitle: 'Paquetes Debian (.deb) Disponibles',
      packages: [
        {
          name: 'podmesh-core',
          version: '2.4.0',
          arch: 'amd64',
          desc: 'Motor de ejecución PodMesh, gobernador de estrato 0 y agente puente.',
          file: 'podmesh-core_2.4.0_amd64.deb',
          size: '14.2 MB',
        },
        {
          name: 'pmbs-server',
          version: '2.4.0',
          arch: 'amd64',
          desc: 'Servidor autónomo de copias desduplicadas y ejecutor de Op 10.',
          file: 'pmbs-server_2.4.0_amd64.deb',
          size: '9.8 MB',
        },
        {
          name: 'meshfs-node',
          version: '2.4.0',
          arch: 'amd64',
          desc: 'Nodo de sistema de archivos distribuido con árbol de Merkle.',
          file: 'meshfs-node_2.4.0_amd64.deb',
          size: '8.4 MB',
        },
        {
          name: 'pve-storage-podmesh',
          version: '2.4.0',
          arch: 'all',
          desc: 'Plugin de almacenamiento en Perl para Proxmox VE (QEMU, LXC, VZDump).',
          file: 'pve-storage-podmesh_2.4.0_all.deb',
          size: '1.2 MB',
        },
      ],
      cliTitle: 'Cheatsheet CLI Interactiva',
      cliTabs: {
        podmesh: 'podmesh',
        pmbs: 'pmbs-cli',
        meshfs: 'meshfs',
      },
    },
  },
}

export const cliSnippets = {
  podmesh: [
    {
      title: 'Lister les univers actifs et leur strate',
      cmd: 'podmesh universe list --format=json',
      desc: 'Affiche tous les univers en cours d’exécution avec leur statut canonique et le nœud hôte.',
    },
    {
      title: 'Relocalisation à chaud en 42ms (Live Migration)',
      cmd: 'podmesh migrate univ-prod-saas-01 --target=node-compute-02 --verify-btrfs',
      desc: 'Transfère l’univers vers le nœud cible sans interruption de trafic ni perte de session.',
    },
    {
      title: 'Vérifier l’état du Quorum Ed25519',
      cmd: 'podmesh quorum status --deep',
      desc: 'Contrôle la validation cryptographique du quorum entre les nœuds fédérés.',
    },
    {
      title: 'Déployer un univers client avec isolation userns',
      cmd: 'podmesh universe deploy --name=boutique-prod --matrix=debian13:443dedd --lanes=4 --env=prod',
      desc: 'Spécialise un nouvel univers hermétique sous cgroups v2 avec injection de clé SSH parente.',
    },
  ],
  pmbs: [
    {
      title: 'Créer un backup certifié avec preuve de survie Op 10',
      cmd: 'pmbs-cli backup create univ-prod-saas-01 --verify-boot-restore --immutable',
      desc: 'Effectue le snapshot, calcule les deltas SHA-256 et déclenche le boot test en sandbox 1.1s.',
    },
    {
      title: 'Tester la survie d’un snapshot existant (Op 10)',
      cmd: 'pmbs-cli sandbox boot-test bkp-958e0b74 --timeout=5s',
      desc: 'Instancie le conteneur sandbox, valide MariaDB et appose le sceau certifié Ed25519.',
    },
    {
      title: 'Time-Travel Fork : Créer une branche de test instantanée',
      cmd: 'pmbs-cli fork bkp-958e0b74 --name=univ-qa-hotfix-02',
      desc: 'Crée un nouvel univers fonctionnel en 310ms par clonage CoW sans duplication d’octets.',
    },
    {
      title: 'Restaurer un fichier unitaire depuis la Time-Machine',
      cmd: 'pmbs-cli restore-file bkp-958e0b74 --path=/etc/shaper/status.json --out=/tmp/restored.json',
      desc: 'Extrait atomiquement un unique fichier d’un snapshot de plusieurs gigaoctets.',
    },
  ],
  meshfs: [
    {
      title: 'Initialiser un pool distribué MeshFS',
      cmd: 'meshfs pool init --name=pool-prod-01 --backend=btrfs --chunk-size=4M /mnt/storage',
      desc: 'Crée l’arbre de Merkle racine et prépare l’adressage CAS sur les blocs de 4MB.',
    },
    {
      title: 'Joindre un nœud au cluster MeshFS (avec ou sans PodMesh)',
      cmd: 'meshfs cluster join --peer=192.168.10.10:9870 --token=mfs_tok_84f9a2e',
      desc: 'Synchronise immédiatement l’arbre de Merkle et active la réplication élastique.',
    },
    {
      title: 'Monter le système de fichiers distribué',
      cmd: 'mount -t meshfs pool-prod-01:/universes /meshfs/universes -o cas_cache=direct',
      desc: 'Point de montage FUSE / Kernel optimisé avec déduplication transparente en lecture/écriture.',
    },
    {
      title: 'Créer un instantané Merkle O(1)',
      cmd: 'meshfs snapshot create /meshfs/universes/univ-demo-saas-prod@snap-20261019',
      desc: 'Gèle l’état en temps réel en inscrivant le hash racine sans bloquer les I/O applicatives.',
    },
  ],
}
