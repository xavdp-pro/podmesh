# PodMesh & MeshFS Architecture Specification
## Pure Rust Sovereign Systems, Distributed Storage, and Proxmox VE Integration

**Language:** English  
**Audience:** Autonomous AI Agents (LLMs), Systems Architects & DevOps Engineers  
**Status:** Certified & Qualified in Laboratory (2026-10-19)  
**Classification:** Core Architectural Reference & Machine-Readable Spec

---

## 1. System Overview & Core Philosophy

PodMesh is a sovereign, lightweight container orchestration and distributed storage platform designed to operate seamlessly across standalone Linux hosts, high-availability multi-node clusters, and virtualization hypervisors (Proxmox VE).

### Core Pillars:
1. **100% Pure Rust Native Footprint**: Zero Python, zero Perl, zero JVM, and zero Ceph runtime dependencies required on host nodes.
2. **Sub-Megabyte Memory Residency**:
   - `meshfs` daemon: **~664 KB RAM** resident (1.7 MB peak).
   - `pmbs` daemon: **~596 KB RAM** resident (1.8 MB peak).
   - `podmeshd` daemon: **~1.6 MB RAM** resident.
3. **Fail-Closed Mandate Governance**: Privileged host modifications require cryptographically verified, monotonic leases. If an operator mandate is missing or expired, tools terminate immediately with **exit code 3** (`ERR_MANDATE_MISSING`) without attempting unprivileged fallback.
4. **Content-Addressed Storage (CAS) & Merkle Trees**: Files and virtual machine disks are chunked into immutable 4 MiB SHA-256 blocks, providing native deduplication, instant Copy-on-Write (CoW) snapshots, and bit-rot resistance.

```
+-------------------------------------------------------------------------+
|                        PODMESH ECOSYSTEM STACK                          |
+-------------------------------------------------------------------------+
| [Layer 4]  Proxmox VE Integration (PVE::Storage::Custom::MeshFSPlugin)   |
|            VM Raw/Qcow2 Disks, CT Rootdirs, VZDump Backups, Snapshots   |
+-------------------------------------------------------------------------+
| [Layer 3]  Storage & Backup Daemons (Pure Rust, Port 7443 & 8007)       |
|            MeshFS (4MB CAS, Merkle Tree) | PMBS (Remote Sync, Op10 Proof)|
+-------------------------------------------------------------------------+
| [Layer 2]  Orchestrator & Host Controller                               |
|            podmeshd (/run/podmesh/api.sock) | podmesh-ctl (Multi-Call)  |
|            Fail-Closed Mandates: decision-follow, publisher-follow      |
+-------------------------------------------------------------------------+
| [Layer 1]  Container Runtime & Kernel Primitives                        |
|            Rootful Podman (Flat & Nested Profiles) | Cgroups v2 | Netns |
+-------------------------------------------------------------------------+
```

---

## 2. Component Directory: What Each Tool Does & Why

### 2.1. `podmeshd` (Host Lifecycle Daemon)
- **Role**: Local systemd service (`podmesh.service`) running as root.
- **IPC Socket**: `/run/podmesh/api.sock` (mode `0600`, root-only Unix Domain Socket).
- **Primary Duties**:
  - Manages Podman containers and pods through raw OCI bindings without daemon bloat.
  - Maintains persistent state in an ACID SQLite database (`/var/lib/podmesh/state.sqlite`).
  - Enforces Cgroups v2 limits (`memory.max`, `cpu.max`, `io.weight`) and network isolation.
  - Manages network effect ledgers, veth routing, and NAT exemptions.

### 2.2. `podmesh-ctl` (Native Multi-Call Dispatcher)
- **Role**: Compiled binary (`/usr/bin/podmesh-ctl`) that also acts as drop-in replacement for 4 previous Python scripts via symlinks or `argv[0]`:
  - `/usr/bin/podmesh-decision-follow` -> `podmesh-ctl`
  - `/usr/bin/podmesh-publisher-follow` -> `podmesh-ctl`
  - `/usr/bin/podmesh-fence` -> `podmesh-ctl`
  - `/usr/bin/podmesh-restore` -> `podmesh-ctl`
- **Subcommands**:
  - `podmesh-ctl ha [declare|rotate|inspect]`: Warm-standby lease failover and Ed25519 takeover proofs.
  - `podmesh-ctl replicate [configure|run|status]`: Incremental data replication between nodes.
  - `podmesh-ctl move <UNIVERSE> <SRC> <DST>`: Live migration with pre-copy and cutover.
  - `podmesh-ctl system follow-decision`: Checks `/etc/podmesh/decision-follow-mandate`.
  - `podmesh-ctl system follow-publisher`: Checks `/etc/podmesh/publisher-follow-mandate`.
- **Fail-Closed Contract**: Exit Code `3` if mandate file is not found or invalid.

### 2.3. `meshfs` (Sovereign Distributed Filesystem)
- **Role**: Ultra-lightweight distributed filesystem daemon (`meshfs.service`) listening on TCP port `7443` and Unix socket `/var/lib/meshfs/meshfs.sock`.
- **Alternative to**: Ceph / CephFS / GlusterFS. Eliminates OSDs, MONs, MGRs, and kernel modules.
- **Key Features**:
  - **4 MiB CAS Storage Engine**: Incoming files or VM disk images are partitioned into fixed 4,194,304-byte chunks stored by SHA-256 hash under `/var/lib/meshfs/chunks/`.
  - **Merkle Tree Inodes**: Directory hierarchies and volume snapshots are represented as cryptographic Merkle Trees. A volume's state is summarized in a single 32-byte root hash.
  - **Zero-Copy CoW Reflinks**: Exploits Linux `FICLONE` / `ioctl_ficlone` on Btrfs/XFS/ZFS for sub-millisecond local snapshotting without duplicating physical disk blocks.
  - **Scale-Out Clustering**: Nodes join with `meshfs join <TARGET_IP>:7443` and discover peers dynamically.
  - **Proxmox VE Storage REST API**: Endpoints under `/api/v1/meshfs/*` handle status reporting, chunk upload/download, volume allocation, and snapshot creation.

### 2.4. `pmbs` (PodMesh Backup Server)
- **Role**: Standalone backup server daemon (`pmbs.service`) listening on TCP port `8007` and Unix socket `/var/lib/pmbs/pmbs.sock`.
- **Alternative to**: Proxmox Backup Server (PBS).
- **Key Features**:
  - SHA-256 CAS deduplicated datastores under `/var/lib/pmbs/`.
  - Incremental remote synchronization between geographically separated sites (e.g. `gbs-p4` ⇄ `med-pmox`).
  - Automated Op 10 Sandbox Boot Proof verification: validates that backed-up containers and VM disks can boot into a disposable namespace before signing verification proofs.

### 2.5. `PVE::Storage::Custom::MeshFSPlugin` (Proxmox VE Storage Plugin)
- **Role**: Official Perl storage driver installed to `/usr/share/perl5/PVE/Storage/Custom/MeshFSPlugin.pm`.
- **Integration**:
  - Proxmox automatically detects custom plugins located in `/usr/share/perl5/PVE/Storage/Custom/`.
  - Declared in `/etc/pve/storage.cfg` as `meshfs: <STORAGE_ID>`.
  - Supports content types: `images`, `rootdir`, `backup`, `iso`, `vztmpl`, `snippets`.
  - Provides native hooks for `alloc_image`, `free_image`, `volume_snapshot`, `volume_snapshot_delete`, and `status`.

---

## 3. Container Topology: Flat vs. Nested Profiles

A common architectural question: *Does PodMesh support standard OCI containers (like Nginx, MariaDB, Alpine) without requiring nested Podman inside?*

**YES.** PodMesh natively supports two distinct operational modes:

| Profile | Target Workload | Privileges | Nested Podman? | Typical Use Cases |
| :--- | :--- | :--- | :--- | :--- |
| **`flat`** *(Default)* | Standard microservices, web apps, databases, workers | Unprivileged (`UserNS`, isolated rootless uid map) | **NO** | Redis, PostgreSQL, Nginx, Python, Node.js, Alpine |
| **`nested`** | Rule 11 Sovereign Sub-Engines, CI/CD runners, build agents | `--privileged` with nested cgroups delegation | **YES** | PodMesh builders (`podmesh-build`), Docker-in-Podman, nested hypervisor containers |

---

## 4. Dual APT Repository Architecture (RFC 822)

Packages are distributed across two independent cryptographically signed APT channels:

### 4.1. Channel `dev` (`trixie-dev`)
- **Target**: Staging nodes, experimental labs, continuous integration.
- **Client Configuration**:
  ```ini
  # /etc/apt/sources.list.d/podmesh-dev.list
  deb [signed-by=/etc/apt/keyrings/xavdp.gpg] https://deb.xavdp.pro/dev trixie-dev main
  ```

### 4.2. Channel `prod` (`trixie-stable`)
- **Target**: Production clusters, validated hypervisors, customer-facing workloads.
- **Client Configuration**:
  ```ini
  # /etc/apt/sources.list.d/podmesh-prod.list
  deb [signed-by=/etc/apt/keyrings/xavdp.gpg] https://deb.xavdp.pro/prod trixie-stable main
  ```

### 4.3. Repository Maintenance
- Generate both channels: `make apt-all`
- Generate dev channel only: `make apt-dev`
- Generate prod channel only: `make apt-prod`

---

## 5. Machine-Readable Specifications (For AI Agents & LLMs)

When an AI agent interacts with the PodMesh system, it must adhere to the following deterministic rules:

### 5.1. IPC Protocol (`/run/podmesh/api.sock`)
- All requests must be formatted as JSON objects containing an `"operation"` field.
- Expected response format:
  ```json
  {
    "ok": true,
    "observed_at": 1792427186,
    "data": { ... }
  }
  ```
- If `"ok": false`, an `"error"` string is guaranteed to be present.

### 5.2. Mandatory Error Codes
- `0`: Success.
- `1`: General execution or operational failure.
- `2`: Invalid CLI syntax or unsupported arguments.
- `3`: `ERR_MANDATE_MISSING` — A required mandate file in `/etc/podmesh/` was missing or unreadable. The agent must NOT retry blindly without inspecting the mandate path.

### 5.3. Fencing & Anti-Split-Brain Invariant
- **Rule**: In any primary/secondary failover (e.g. A0 ⇄ A1), the primary site MUST be confirmed stopped and excluded before the secondary site is issued an activation lease.
- **Verification**: If `controller.py status` or `podmesh-ctl ha inspect` reports an unresolved lock or ambiguous active lease, failover commands must be aborted to protect data integrity.

---

## 6. Human Operator Runbook & Cheatsheet

### 6.1. Service Health & Inspection
```bash
# Check status of local daemons
systemctl status podmesh meshfs pmbs

# Query local MeshFS node and cluster health
meshfs ping
meshfs status

# Query PMBS datastores
pmbs status
```

### 6.2. MeshFS File & Storage Operations
```bash
# Upload a file into MeshFS (automatically split into 4MB CAS chunks)
meshfs put /data/my-disk.raw /tmp/local-disk.raw

# Create an instant Merkle Tree snapshot
meshfs snapshot -v default -n snap-2026-10-19

# Run cryptographic anti-bit-rot audit
meshfs inspect

# Retrieve a file bit-for-bit
meshfs get /data/my-disk.raw -o /tmp/restored-disk.raw
```

### 6.3. Proxmox VE Storage Registration
```bash
# Install and register the MeshFS PVE plugin
/usr/share/meshfs/proxmox/install-pve-plugin.sh

# Check that Proxmox reports the storage pool as active
pvesm status --storage meshfs

# Allocate a 10 GiB virtual disk for VM 2000 on MeshFS
pvesm alloc meshfs 2000 vm-2000-disk-1.raw 10G

# List volumes
pvesm list meshfs

# Remove a volume
pvesm free meshfs:2000/vm-2000-disk-1.raw
```

### 6.4. High-Availability & Standby Failover
```bash
# Rotate universe authority epoch with Ed25519 proof
podmesh-ctl ha rotate my-universe --destination-node lab-b

# Manually trigger return to primary on resilience lab
pct exec 9404 -- python3 /opt/gbs-resilience-lab/automation/controller.py return-primary
```
