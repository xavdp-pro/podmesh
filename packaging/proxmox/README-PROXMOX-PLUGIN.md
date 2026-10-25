# Plugin de Stockage Proxmox VE pour PodMesh MeshFS

> **Stockage distribué souverain, ultra-léger et autonome avec déduplication SHA-256 et snapshots Merkle Copy-on-Write pour Proxmox VE.**  
> *L'alternative souveraine et frugale à CephFS : fonctionne de 1 nœud autonome à N nœuds en cluster sans la lourdeur d'administration de Ceph.*

---

## 1. Vue d'Ensemble & Proposition de Valeur

Dans un cluster de virtualisation Proxmox VE, le stockage distribué partagé est généralement confié à **Ceph / CephFS**. Si Ceph est robuste à très grande échelle, il souffre de contraintes majeures pour les infrastructures souveraines, les edge datacenters et les architectures frugales :
- **Consommation excessive de RAM/CPU** (démons MON, OSD, MGR, MDS).
- **Complexité d'exploitation** (CRUSH map, PGs, rééquilibrage réseau).
- **Inadéquation sur 1 ou 2 nœuds** (quorum Ceph instable sans arbitre).

**PodMesh MeshFS** résout ce défi en offrant :
1. **Légèreté absolue** : Démon autonome en Python 3 sans aucune dépendance lourde.
2. **Élasticité de 1 à N nœuds** : Fonctionne parfaitement sur une node Proxmox unique, puis s'étend naturellement en multi-nodes.
3. **Déduplication globale SHA-256** : Chunks canoniques de 4 Mo dédupliqués automatiquement au stockage.
4. **Snapshots Merkle CoW instantanés** : Arbre de Merkle cryptographique garantissant l'intégrité et des snapshots instantanés.
5. **Intégration Proxmox VE native** : Plugin Perl héritant de `PVE::Storage::Plugin` et reconnu par l'UI Proxmox, `pvesm`, `qm` et `pct`.
6. **Live Migration (`shared 1`)** : Migration à chaud des machines virtuelles KVM et conteneurs LXC d'un nœud Proxmox à l'autre sans interruption de service.

---

## 2. Architecture du Plugin

Le plugin Proxmox VE se compose de :
- **`/usr/share/perl5/PVE/Storage/Custom/MeshFSPlugin.pm`** : Le module Perl chargé dynamiquement par le moteur Proxmox (`PVE::Storage`).
- **`packaging/proxmox/install-pve-plugin.sh`** : Le script d'installation automatisé 1-clic.

### Diagramme d'Intégration Proxmox VE

```
+-------------------------------------------------------------------+
|                        Proxmox VE Host                            |
|                                                                   |
|  [ GUI Web / API ]        [ qm / pct CLI ]       [ pvesm tool ]   |
|          \                       |                     /          |
|           +----------------------+--------------------+           |
|                                  |                                |
|                        PVE::Storage Core Engine                   |
|                                  |                                |
|              +-------------------+-------------------+            |
|              | PVE::Storage::Custom::MeshFSPlugin.pm |            |
|              +-------------------+-------------------+            |
|                                  |                                |
|          +-----------------------+-----------------------+        |
|          | Local POSIX / FUSE Path                       |        |
|          | /mnt/pve/meshfs/images/<vmid>/...             |        |
|          |                                               |        |
|          | REST API (HTTP 7443)                          |        |
|          | /api/v1/meshfs/status                         |        |
|          | /api/v1/meshfs/snapshots                      |        |
+----------+-----------------------------------------------+--------+
                                   |
           +-----------------------+-----------------------+
           |           PodMesh MeshFS Daemon               |
           |      (meshfs.service sur le port 7443)        |
           |                                               |
           |  - ChunkStore SHA-256 (/var/lib/meshfs/chunks)|
           |  - Merkle Tree Snapshots                      |
           |  - Synchronisation inter-nœuds MeshFS         |
           +-----------------------------------------------+
```

---

## 3. Hooks Proxmox VE Implémentés

Le plugin implémente l'ensemble des hooks de l'API de stockage Proxmox VE (API version 16, compatible PVE 7.x et 8.x) :

| Méthode Hook | Description & Comportement MeshFS |
|---|---|
| `type()` | Retourne `'meshfs'` comme identifiant unique du driver de stockage. |
| `api()` | Expose la version de l'API Storage supportée (`APIVER 16`, rétrocompatible). |
| `plugindata()` | Déclare les formats supportés (`raw`, `qcow2`, `subvol`) et les types de contenus autorisés. |
| `properties()` & `options()` | Configure les attributs du stockage : `server`, `port`, `volume`, `path`, `shared`, `content`, `nodes`, `mountoptions`. |
| `check_config()` | Définit par défaut le point de montage local (`/mnt/pve/<storage_id>`) si non spécifié. |
| `activate_storage()` | Interroge l'API MeshFS (`/api/v1/meshfs/status`), valide la connectivité et initialise l'arborescence Proxmox (`images`, `dump`, `iso`, `template`). |
| `deactivate_storage()` | Assure le démontage propre FUSE / umount si le chemin était monté. |
| `status()` | Interroge `/api/v1/meshfs/status` et `PVE::Tools::df` pour retourner `(total, free, used, active)`. |
| `alloc_image()` | Alloue l'image de disque virtuel (`qemu-img create`) et déclare le volume auprès de l'API MeshFS. |
| `free_image()` | Supprime l'image disque physique et purge les snapshots Merkle orphelins associés. |
| `list_images()` | Énumère l'ensemble des disques VM/LXC alloués sur le stockage. |
| `volume_has_feature()` | Déclare le support natif des fonctionnalités `snapshot`, `clone`, `copy` et `template`. |
| `volume_snapshot()` | Déclenche un snapshot d'arbre de Merkle instantané sur l'API MeshFS (`POST /api/v1/meshfs/snapshots`) et crée le snapshot qemu-img. |
| `volume_snapshot_delete()` | Supprime le snapshot Merkle MeshFS (`POST /api/v1/meshfs/snapshots/delete`) et le snapshot qemu-img. |
| `volume_snapshot_rollback()` | Restaure l'état du disque à partir du snapshot sélectionné. |

---

## 4. Types de Contenus Supportés

Le stockage MeshFS supporte l'intégralité des flux de travail Proxmox VE :
- **`images`** : Disques durs des machines virtuelles KVM (`.qcow2`, `.raw`).
- **`rootdir`** : Systèmes de fichiers racine pour conteneurs LXC.
- **`backup`** : Archives de sauvegarde VZDump Proxmox (`.vma.zst`, `.tar.zst`).
- **`iso`** : Images ISO d'installation d'OS.
- **`vztmpl`** : Modèles d'appareils et appliances LXC.
- **`snippets`** : Fichiers de configuration personnalisés et scripts Cloud-Init.

---

## 5. Déploiement & Installation 1-Clic

### Option A : Installation Automatisée via `install-pve-plugin.sh`

Le script d'installation s'occupe de toutes les étapes (vérification des dépendances Perl, copie du module, création des répertoires, déclaration `/etc/pve/storage.cfg` et rechargement des services) :

```bash
cd /path/to/podmesh/packaging/proxmox
sudo ./install-pve-plugin.sh --storage-id meshfs --server 127.0.0.1 --port 7443 --shared 1
```

#### Options disponibles :
```text
  -s, --storage-id ID     Nom de l'identifiant de stockage Proxmox (défaut: meshfs)
  -h, --server IP/HOST    Adresse du nœud MeshFS (défaut: 127.0.0.1)
  -p, --port PORT         Port de l'API REST MeshFS (défaut: 7443)
  -v, --volume VOL        Nom du volume MeshFS (défaut: default)
  -m, --path CHEMIN       Chemin local du montage (défaut: /mnt/pve/<storage-id>)
      --shared 0|1        Activer le mode partagé cluster (défaut: 1)
      --content LISTE     Types de contenus autorisés (défaut: images,rootdir,backup,iso,vztmpl,snippets)
      --dry-run           Mode simulation sans modification
```

### Option B : Installation Manuelle Étape par Étape

1. **Installer les modules Perl requis** :
   ```bash
   apt-get update
   apt-get install -y libwww-perl libjson-perl
   ```

2. **Copier le plugin dans l'arborescence Proxmox** :
   ```bash
   mkdir -p /usr/share/perl5/PVE/Storage/Custom
   cp MeshFSPlugin.pm /usr/share/perl5/PVE/Storage/Custom/MeshFSPlugin.pm
   chmod 644 /usr/share/perl5/PVE/Storage/Custom/MeshFSPlugin.pm
   ```

3. **Créer le point de montage local** :
   ```bash
   mkdir -p /mnt/pve/meshfs/{images,dump,template/iso,template/cache,snippets}
   chmod 755 /mnt/pve/meshfs
   ```

4. **Déclarer le stockage dans `/etc/pve/storage.cfg`** :
   ```text
   meshfs: meshfs
   	path /mnt/pve/meshfs
   	server 127.0.0.1
   	port 7443
   	volume default
   	content images,rootdir,backup,iso,vztmpl,snippets
   	shared 1
   ```

5. **Recharger les services Proxmox VE** :
   ```bash
   systemctl reload-or-restart pvedaemon pveproxy pvestatd
   ```

---

## 6. Guide d'Exploitation & Commandes CLI

### Vérification de l'état du stockage

```bash
# Vérifier la présence et l'état du stockage dans Proxmox
pvesm status --storage meshfs

# Lister les disques alloués
pvesm list meshfs

# Vérifier l'API MeshFS directement
curl -s http://127.0.0.1:7443/api/v1/meshfs/status | jq .
```

### Création d'une Machine Virtuelle sur MeshFS

```bash
# Création d'une VM 100 avec un disque de 20 Go sur MeshFS
qm create 100 \
  --name "vm-meshfs-demo" \
  --memory 2048 \
  --cores 2 \
  --scsihw virtio-scsi-pci \
  --scsi0 meshfs:20,format=qcow2 \
  --net0 virtio,bridge=vmbr0
```

### Snapshots Merkle Copy-on-Write

```bash
# Créer un snapshot instantané
qm snapshot 100 snap-update-kernel --description "Snapshot avant MAJ kernel"

# Lister les snapshots du démon MeshFS
curl -s http://127.0.0.1:7443/api/v1/meshfs/snapshots | jq .

# Restaurer le snapshot
qm rollback 100 snap-update-kernel

# Supprimer le snapshot
qm delsnapshot 100 snap-update-kernel
```

### Migration à chaud (Live Migration)

Grâce à la directive `shared 1`, Proxmox VE sait que le stockage MeshFS est partagé entre les nœuds du cluster :
```bash
# Migration de la VM 100 vers le nœud pve2 sans éteindre la VM
qm migrate 100 pve2 --online
```

---

## 7. Dépannage & Diagnostics

| Symptôme | Cause Probable | Action Corrective |
|---|---|---|
| `storage 'meshfs' is not active` | Démon MeshFS non démarré ou port inaccessible. | Exécuter `systemctl status meshfs` et vérifier que le port 7443 est en écoute. |
| `Compilation error: get_standard_format` | Mauvais import dans Perl. | Utiliser `PVE::JSONSchema qw(get_standard_option)`. Déjà corrigé dans la version actuelle. |
| `API version too old` | Évolution majeure de Proxmox VE. | La méthode `api()` retourne dynamiquement `PVE::Storage::APIVER()`. |
| `Snapshot failed: qemu-img error` | Permissions sur le fichier image ou format incompatible. | S'assurer que le fichier `.qcow2` est accessible en lecture/écriture par `root`. |

---

## 8. Conclusion & Prochaines Étapes

Le plugin `PVE::Storage::Custom::MeshFSPlugin` transforme **MeshFS** en un backend de stockage unifié de classe entreprise pour Proxmox VE. Il élimine la complexité de Ceph tout en conservant les avantages clés : haute disponibilité, partage multi-nœuds, déduplication et snapshots cryptographiques.
