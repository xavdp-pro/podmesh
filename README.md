# PodMesh — Sovereign Infrastructure, Distributed Storage & Backup Suite

> **100% Pure Rust Architecture • Zero Python Runtime on Storage Nodes • Sub-8MB Footprint**  
> Portable Podman operations for human-agent tandems, high-performance distributed storage (**MeshFS**), deduplicated backup server (**PMBS**), and unified orchestration CLI (**`podmesh-ctl`**).

---

## 🏛 Architecture Globale

PodMesh unifie l'orchestration de conteneurs, le stockage distribué souverain et la sauvegarde continue dans un monorepo Cargo unifié sans dépendances d'exécution externes :

```
                               ┌─────────────────────────────────┐
                               │  Vite / React 19 Showcase / UI  │
                               │        (:5173 - Shaper OS)      │
                               └────────────────┬────────────────┘
                                                │ REST / Sockets
        ┌───────────────────────────────────────┼───────────────────────────────────────┐
        │                                       │                                       │
        ▼                                       ▼                                       ▼
┌───────────────────────┐       ┌───────────────────────────────┐       ┌───────────────────────┐
│     MeshFS Daemon     │       │       PodMesh Node Engine     │       │      PMBS Daemon      │
│   (crates/meshfs)     │       │       (src/ - Core Engine)    │       │     (crates/pmbs)     │
├───────────────────────┤       ├───────────────────────────────┤       ├───────────────────────┤
│ • CAS SHA-256 4MB     │       │ • Rootful Podman Lifecycle    │       │ • CAS SHA-256 Dedup   │
│ • Merkle DAG Inodes   │       │ • MariaDB & SQLite Stores     │       │ • Op 10 Boot Proof    │
│ • ioctl FICLONE CoW   │       │ • Reconcile & State Machine   │       │ • Cross-Site Sync     │
│ • Proxmox VE Plugin   │       │ • Ed25519 Takeover Proofs     │       │ • Single-File Restore │
│ • Port 7443 + IPC Sock│       │ • Port 8080 + IPC Socket      │       │ • Port 8007 + IPC Sock│
│ • Footprint: < 8MB RAM│       │ • Footprint: < 15MB RAM       │       │ • Footprint: < 8MB RAM│
└───────────────────────┘       └───────────────────────────────┘       └───────────────────────┘
                                                ▲
                                                │ Local / Remote Control
                               ┌────────────────┴────────────────┐
                               │       podmesh-ctl (CLI Rust)    │
                               │       (crates/podmesh-cli)      │
                               └─────────────────────────────────┘
```

---

## 🚀 Démarrage Rapide : Mode Dev vs Mode Prod

Le projet dispose de deux modes de fonctionnement unifiés pilotés par Makefile ou scripts Bash :

### 1. Mode Développement (`make dev`)

Le **Mode Développeur** lance l'ensemble des services en local de manière totalement isolée, sans nécessiter de privilèges `root`, sans modifier `/var/lib`, et avec logs détaillés `--debug` :

```bash
# Lancement de l'environnement complet de développement
make dev

# Ou via le script direct :
./scripts/dev.sh
```

**Composants démarrés automatiquement :**
- **MeshFS Daemon** : API HTTP `http://127.0.0.1:7443` | Socket IPC `./data/dev/meshfs/meshfs.sock`
- **PMBS Daemon**   : API HTTP `http://127.0.0.1:8007` | Socket IPC `./data/dev/pmbs/pmbs.sock`
- **Showcase UI**   : Interface Web interactive `http://localhost:5173`
- **Logs en direct** : `tail -f data/dev/*.log`

> **Note :** Pour tester automatiquement le démarrage et l'arrêt propre sans intervention :
> ```bash
> make dev-check
> ```

---

### 2. Mode Production (`make prod`)

Le **Mode Production** compile l'ensemble du workspace Rust avec optimisations maximales (`opt-level=3`, LTO agressif, `strip=true`, `panic="abort"`), assemble le bundle web Vite, et génère tous les paquets Debian autonomes (`.deb`) dans `dist/` avec contrôle d'empreintes SHA-256 :

```bash
# Compilation release et génération des paquets Debian
make prod

# Ou via le script direct :
./scripts/prod.sh
```

**Artefacts générés dans `dist/` :**
| Paquet Debian | Rôle & Description | Taille |
|---|---|---|
| `podmesh-meshfs_2.1.0_amd64.deb` | Moteur MeshFS Pure Rust + Plugin Proxmox VE + Service systemd | ~1.7 MB |
| `podmesh-backup-server_2.1.0_amd64.deb` | Serveur de sauvegarde PMBS Pure Rust + Service systemd | ~1.6 MB |
| `podmesh-cli_2.1.0_amd64.deb` | CLI unifié `podmesh-ctl` autonome (HA, failover, replication) | ~600 KB |
| `podmesh_0.1.0~experimental7_amd64.deb` | Moteur PodMesh Node complet (`podmesh`, `podmeshd`, `podmesh-ctl`) | ~2.4 MB |

---

### 🌐 3. Deux Canaux de Repositories Debian APT (DEV & PROD)

Le système intègre un générateur complet d'arborescences de dépôts Debian APT standards (`pool/main`, `dists/.../Packages.gz`, `Release`) scindé en deux canaux distincts :

#### 🟡 Canal DÉVELOPPEMENT (`trixie-dev`)
Dédié aux versions expérimentales, aux tests d'intégration continue et aux pré-versions :
```bash
# Générer le canal APT Dev
make apt-dev

# Fichier client sur les nœuds de test (/etc/apt/sources.list.d/podmesh-dev.list) :
deb [signed-by=/etc/apt/keyrings/xavdp.gpg] https://deb.xavdp.pro/dev trixie-dev main
```

#### 🟢 Canal PRODUCTION (`trixie-stable`)
Dédié aux déploiements stables sur les hyperviseurs Proxmox VE et serveurs de production :
```bash
# Générer le canal APT Prod
make apt-prod

# Fichier client sur les nœuds de production (/etc/apt/sources.list.d/podmesh-prod.list) :
deb [signed-by=/etc/apt/keyrings/xavdp.gpg] https://deb.xavdp.pro/prod trixie-stable main
```

> **Note :** La commande `make apt-all` indexe et génère simultanément les deux canaux dans `dist/apt-repos/`.

---

## 📦 Commandes du Makefile

| Commande | Action |
|---|---|
| `make dev` | Démarre l'environnement de dev local (MeshFS + PMBS + Showcase) |
| `make dev-check` | Vérifie la santé et l'auto-démarrage du mode dev en non-interactif |
| `make prod` | Compile en Release (LTO) et génère tous les paquets `.deb` |
| `make packages` | Construit uniquement les paquets Debian dans `dist/` |
| `make test` | Exécute l'intégralité des 185+ tests unitaires du workspace Rust |
| `make check` | Vérification syntaxique et de types `cargo check --workspace` |
| `make health` | Audit de connectivité HTTP et vérification des Sockets Unix IPC |
| `make clean` | Nettoie les binaires compilés, les logs et données volatiles dev |

---

## 🛠 Déploiement en Production sur Nœuds Proxmox / Debian

Sur un serveur Debian 12/13 ou Proxmox VE :

```bash
# 1. Installation de MeshFS (Stockage Distribué & Plugin PVE automatique)
sudo dpkg -i dist/podmesh-meshfs_2.1.0_amd64.deb

# 2. Installation de PMBS (Backup Server dédupliqué)
sudo dpkg -i dist/podmesh-backup-server_2.1.0_amd64.deb

# 3. Installation de PodMesh Engine & CLI
sudo dpkg -i dist/podmesh_0.1.0~experimental7_amd64.deb

# 4. Vérification de l'état des services systemd
sudo systemctl status meshfs.service
sudo systemctl status pmbs.service
sudo systemctl status podmesh.service
```

### Intégration Native Proxmox VE :
Le paquet `podmesh-meshfs` installe automatiquement `MeshFSPlugin.pm` dans `/usr/share/perl5/PVE/Storage/Custom/MeshFSPlugin.pm` et l'enregistre auprès de `pvedaemon` / `pveproxy`. MeshFS apparaît immédiatement comme stockage sélectionnable pour :
- Images disques QEMU (`images`)
- Conteneurs LXC (`rootdir`)
- Sauvegardes VZDump (`backup`)
- ISOs et templates (`iso`, `vztmpl`)

---

## ⚡ CLI Unifié `podmesh-ctl`

Le binaire `podmesh-ctl` (100% Rust) remplace les anciens scripts Python et shell :

```bash
# Déclaration de politique HA pour un univers
podmesh-ctl ha declare my-universe --authority authority-quorum-1 --lease 30

# Rotation d'époque et génération de preuve Ed25519 de reprise
podmesh-ctl ha rotate my-universe --host gbs-p4

# Configuration de la réplication inter-nœuds
podmesh-ctl replicate configure my-universe --active gbs-p4 --standbys med-pmox --interval 300

# Déplacement à chaud d'un univers
podmesh-ctl move my-universe gbs-p4 med-pmox

# Statut du manager de cluster
podmesh-ctl admin status
```

---

## 🔒 Confidentialité & Intégrité
- **Cryptographie** : Preuves de reprise et certificats signés en **Ed25519**, déduplication et intégrité CAS via **SHA-256**.
- **Bit-Rot Protection** : `meshfs inspect` vérifie cryptographiquement chaque chunk 4MB en lecture directe.
- **Souveraineté** : Zéro télémétrie, zéro dépendance cloud propriétaire.

---

## Licence
Sous licence Apache-2.0. Créé par Xavier de Poorter et l'équipe d'ingénieurs PodMesh.

## Dedication

[Personal dedication by Xavier de Poorter](DEDICATION.md).
