# AI-First Installation, Bootstrap & Configuration Protocol — PodMesh v2.3.0

> **Core Directive for Autonomous AI Agents (Claude Code, Codex, Antigravity, Muse, SRE Agents)**:
> PodMesh v2.3.0 and all its satellites are architected as **100% AI-First, headless-native infrastructure**.
> **Zero web interface is required or used** to install, bootstrap, configure, join, replicate, backup, restore, or audit a PodMesh node or cluster. Web interfaces (`podmesh-cockpit`, `podmesh-docs`) are strictly optional/deferred visual layers.
> Every operation is deterministically driven via CLI subcommands, environment variables, declarative JSON/systemd configurations, Unix Domain Sockets (`0600`/`0700`), and loopback JSON REST endpoints (`127.0.0.1`).

---

## 1. Governing Operational Laws for AI Agents

Before executing any installation or configuration command on a target host, the AI agent MUST enforce these four invariants:

1. **Rule 0G (Zero Mocks / Real Iron Only)**: Never simulate, stub, or fake daemon states, storage mounts, or backup manifests. Every verification step must query the live binary, Unix socket, or kernel mount table.
2. **Rule 38 (Bounded Retry & Fail-Closed Escalation, $k \le 2$)**:
   - If a build, service start, socket probe, or cluster join fails, the agent may attempt at most **$k \le 2$ deterministic corrective actions** (e.g., fixing a missing directory permission or restarting a unit after config reload).
   - Upon a second consecutive failure ($k = 2$), the agent MUST **halt immediately (fail-closed)**, leave no zombie processes or partial lockfiles, and emit a structured JSON diagnostic report to the operator.
3. **GEM-001 (Zero Public Unauthenticated Ports)**:
   - Daemons bind to Unix Domain Sockets (`/run/podmesh/api.sock`, `/var/lib/meshfs/meshfs.sock`, `/var/lib/pmbs/pmbs.sock`) or loopback/WireGuard interfaces (`127.0.0.1` or `wg-podmesh` peer IP). Never expose unauthenticated ports on `0.0.0.0`.
4. **Sanctuary Boundary**:
   - When operating on nested qualification clusters (e.g., Proxmox VMs), never install or mutate packages on the physical hypervisor host OS unless explicitly instructed by the operator.

---

## 2. Ecosystem Deployment Order (The 5 Infrastructure Bricks)

When an AI agent provisions a complete PodMesh v2.3.0 node or Proxmox VE cluster from scratch, it deploys the repositories in this exact dependency order:

| Step | Component Repository | Binary / Artifact | Control Socket / Endpoint | AI Configuration Guide |
| :---: | :--- | :--- | :--- | :--- |
| **1** | [`xavdp-pro/podmesh-installer`](https://github.com/xavdp-pro/podmesh-installer) | `podmesh_installerd.py` / `.deb` | `/tmp/podmesh-installer.sock` (JSON IPC) | `podmesh-installer/AI_INSTALL_AND_CONFIG.md` |
| **2** | [`xavdp-pro/podmesh-meshfs`](https://github.com/xavdp-pro/podmesh-meshfs) | `/usr/local/bin/meshfs` | `/var/lib/meshfs/meshfs.sock` & `127.0.0.1:7443` | `podmesh-meshfs/AI_INSTALL_AND_CONFIG.md` |
| **3** | [`xavdp-pro/podmesh-pve-plugin`](https://github.com/xavdp-pro/podmesh-pve-plugin) | `MeshFSPlugin.pm` *(Proxmox only)* | `pvesm` CLI & `/etc/pve/storage.cfg` | `podmesh-pve-plugin/AI_INSTALL_AND_CONFIG.md` |
| **4** | [`xavdp-pro/podmesh-pmbs`](https://github.com/xavdp-pro/podmesh-pmbs) | `/usr/local/bin/pmbs` | `/var/lib/pmbs/pmbs.sock` & `127.0.0.1:8007` | `podmesh-pmbs/AI_INSTALL_AND_CONFIG.md` |
| **5** | [`xavdp-pro/podmesh`](https://github.com/xavdp-pro/podmesh) *(this repo)* | `podmeshd`, `podmesh-ctl` | `/run/podmesh/api.sock` & `/var/lib/podmesh` | Sections 3–6 below |

*(Note: `podmesh-cockpit` and `podmesh-docs` are web surfaces and are intentionally excluded from core infrastructure deployment.)*

---

## 3. Step-by-Step AI Installation of `podmesh` Core (`v2.3.0`)

### 3.1 Pre-Flight Verification

Run the following non-destructive probe to verify OS, kernel, and runtime prerequisites (Debian 13 Trixie / Proxmox VE 9.x):

```bash
set -euo pipefail
uname -a
command -v podman >/dev/null && podman --version
command -v wg >/dev/null && wg --version
command -v btrfs >/dev/null && btrfs --version
```

If missing on a target node, install the base headless packages non-interactively:

```bash
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq podman wireguard-tools btrfs-progs fuse3 curl jq ca-certificates
```

### 3.2 Building or Installing the Binaries

From the `podmesh` repository root (or build host):

```bash
# Compile the entire workspace in release mode (produces podmeshd, podmesh-ctl, meshfs, pmbs)
cargo build --release --workspace

# Verify compiled versions
./target/release/podmesh-ctl --version
./target/release/meshfs --version
./target/release/pmbs --version
```

Install the binaries onto the target host with strict permissions:

```bash
install -m 0755 target/release/podmeshd /usr/bin/podmeshd
install -m 0755 target/release/podmesh /usr/bin/podmesh
install -m 0755 target/release/podmesh-ctl /usr/local/bin/podmesh-ctl
install -m 0755 target/release/podmesh-host-adapter /usr/bin/podmesh-host-adapter
install -m 0755 target/release/podmesh-store-admin /usr/bin/podmesh-store-admin
install -m 0755 target/release/podmesh-storage-migrate /usr/bin/podmesh-storage-migrate
```

*(Alternatively, build and install the `.deb` packages via `./packaging/build-all-debs.sh` and `dpkg -i`.)*

---

## 4. Declarative Headless Configuration (`podmeshd` & `podmesh-ctl`)

### 4.1 Directory & State Layout

Create the state and runtime directories with strict `0700` permissions:

```bash
install -d -m 0700 /var/lib/podmesh
install -d -m 0700 /var/lib/podmesh/ha
install -d -m 0700 /var/lib/podmesh/replication
install -d -m 0700 /run/podmesh
install -d -m 0750 /etc/podmesh
```

### 4.2 Environment Configuration (`/etc/podmesh/podmesh.env`)

Create `/etc/podmesh/podmesh.env` (`chmod 0600`):

```ini
# PodMesh v2.3.0 Headless AI-First Node Configuration
PODMESH_NODE_ID=node-a
PODMESH_STATE_DIR=/var/lib/podmesh
PODMESH_SOCKET=/run/podmesh/api.sock
PODMESH_MESHFS_SOCKET=/var/lib/meshfs/meshfs.sock
PODMESH_MESHFS_URL=http://127.0.0.1:7443
PODMESH_PMBS_SOCKET=/var/lib/pmbs/pmbs.sock
PODMESH_PMBS_URL=http://127.0.0.1:8007
RUST_LOG=info
```

### 4.3 Systemd Service Activation

Install and enable the systemd units:

```bash
cp packaging/podmesh.service /etc/systemd/system/podmesh.service
cp packaging/podmesh-fence.service packaging/podmesh-fence.timer /etc/systemd/system/
cp packaging/podmesh-decision-follow.service packaging/podmesh-decision-follow.timer /etc/systemd/system/
cp packaging/podmesh-restore.service /etc/systemd/system/

systemctl daemon-reload
systemctl enable --now podmesh.service
```

---

## 5. AI-Driven Operations via `podmesh-ctl` (100% JSON Output)

Every `podmesh-ctl` subcommand outputs deterministic, pretty-printed JSON to `stdout` and returns exit code `0` on success or `1` on failure. AI agents parse `stdout` directly with `jq` or native JSON parsers.

### 5.1 Cluster & Node Status Probe

```bash
podmesh-ctl admin status
```

Expected JSON output structure:

```json
{
  "status": "nominal",
  "version": "2.3.0",
  "ledger_dir": "/var/lib/podmesh"
}
```

### 5.2 High-Availability (HA) Policy Declaration & Epoch Rotation

1. **Declare HA authority and lease parameters for a universe**:
   ```bash
   podmesh-ctl ha declare univ-prod \
     --authority authority-main \
     --lease 30 \
     --margin 20
   ```
2. **Inspect current HA epoch and lease state**:
   ```bash
   podmesh-ctl ha inspect univ-prod
   ```
3. **Rotate epoch to a target host (produces Ed25519 takeover proof)**:
   ```bash
   podmesh-ctl ha rotate univ-prod --host node-b --barrier 1
   ```

### 5.3 Universe Replication Configuration & Execution

1. **Configure active-to-standby replication**:
   ```bash
   podmesh-ctl replicate configure univ-prod \
     --active node-a \
     --standbys node-b,node-c \
     --interval 900 \
     --live
   ```
2. **Trigger an immediate replication cycle**:
   ```bash
   podmesh-ctl replicate run univ-prod
   ```
3. **Query replication status**:
   ```bash
   podmesh-ctl replicate status univ-prod
   ```

### 5.4 Live Universe Relocation (`move`)

Relocate a running universe between two nodes with atomic cutover:

```bash
podmesh-ctl move univ-prod node-a node-b
```

### 5.5 Self-Fencing & Boot Restore Mandates

```bash
# Execute deterministic self-fencing from a JSON mandate file
podmesh-ctl system fence /etc/podmesh/mandate-fence.json

# Execute boot-time universe restore from a JSON mandate file
podmesh-ctl system restore /etc/podmesh/mandate-restore.json
```

---

## 6. Private MariaDB Functional Units (`podmesh-node` & `podmesh-manager`)

Under the `SEP22-CONTAINER-MARIADB` profile, durable state for `podmesh-node` and `podmesh-manager` runs inside isolated rootless Podman units communicating exclusively over Unix domain sockets (zero TCP database listener):

- **Node Unit**: `packaging/podmesh-node/private-unit/`
- **Manager Unit**: `packaging/podmesh-manager/private-unit/`
- **Qualification Driver**: `packaging/podmesh-node/qualification/`

To initialize or verify the private unit store via CLI:

```bash
/usr/bin/podmesh-store-admin --help
```

---

## 7. Complete AI Post-Flight Verification Script

An AI agent finishing installation on any node MUST run this verification block and confirm `"ok": true`:

```bash
#!/usr/bin/env bash
set -euo pipefail

echo "[1/4] Verifying CLI binaries..."
podmesh-ctl --version
meshfs --version
pmbs --version

echo "[2/4] Probing MeshFS daemon..."
meshfs ping

echo "[3/4] Probing PMBS daemon..."
pmbs ping

echo "[4/4] Probing PodMesh cluster status..."
podmesh-ctl admin status
```
