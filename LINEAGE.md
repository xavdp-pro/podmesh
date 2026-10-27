# Lineage — podmesh

## Sovereign Orchestrator & SHAPER PHOENIX Modular Split — v2.3.0 (2026-10-27)

- **Current Version**: `2.3.0`
- **Release Date**: `2026-10-27`
- **Canonical Repository**: [`xavdp-pro/podmesh`](https://github.com/xavdp-pro/podmesh)
- **Private Workshop Companion**: [`xavdp-pro/podmesh-lab`](https://github.com/xavdp-pro/podmesh-lab)
- **Governing Standard**: [`xavdp-pro/SHAPER-OS-V1.15`](https://github.com/xavdp-pro/SHAPER-OS-V1.15) (`SEP22-CONTAINER-MARIADB`, Rules 4, 11, 26, 38, GEM-001)

---

## Evolutionary Milestones

### 1. `v0.1.0` — Initial PodMesh Node Engine & Local Lifecycle Contract
- Established the core Rust PodMesh node engine (`src/`), typed operation ledger, SQLite/MariaDB store abstraction, and Unix Domain Socket lifecycle API.
- Defined the human–AI tandem operational contract (`INTENT.md`), the Ideal Scene (`docs/IDEAL-SCENE.md`), and the Host Adapter boundary (`src/host_adapter.rs`).

### 2. `v2.0.0` — Replicated Manager, Ed25519 Epoch Fencing & MariaDB Functional Units
- Introduced single-writer monotonic epoch fencing backed by Ed25519-signed takeover certificates and authority quorum policy digests.
- Implemented canonical functional units (`podmesh-node` and `podmesh-manager`) under the SHAPER `SEP22-CONTAINER-MARIADB` profile with private per-unit MariaDB instances, logical state transfer, and sealed-source restore verification.

### 3. `v2.1.0` — 100% Pure Rust Monorepo Unification (`meshfs`, `pmbs`, `podmesh-ctl`)
- Unified the sovereign infrastructure suite in Pure Rust:
  - `crates/meshfs`: 4 MB SHA-256 Content-Addressable Storage (CAS) distributed filesystem with Merkle CoW snapshots, OpenRaft consensus, and FUSE POSIX mounting.
  - `crates/pmbs`: PodMesh Backup Server with SHA-256 deduplication, authenticated immutable snapshots, and Op 10 sandbox boot proof verification.
  - `crates/podmesh-cli`: Unified `podmesh-ctl` CLI replacing legacy Python and shell operational scripts.
- Added dual-channel Debian APT repository generation (`trixie-dev` and `trixie-stable`).

### 4. `v2.2.0` — 100% Physical Iron Qualification (34/34 Criteria)
- Completed full-coverage laboratory and physical iron qualification (**34/34 Ideal Scene criteria**, including `lifecycle:host-reboot`, Proof 39 MeshFS SLA, Proof 40 Universe Stream, Proof 41 HA Fencing, and monthly observations partition administration).
- Consolidated the bilingual interactive documentation and validation showcase (`showcase/`).

### 5. `v2.3.0` — SHAPER PHOENIX Modular Ecosystem Split
- Under the **SHAPER PHOENIX** canonical architecture, the specialized subsystems developed and qualified inside `podmesh` are published as autonomous, composable `podmesh-*` repositories, each paired with its own dedicated private `-lab` workshop:
  - [`xavdp-pro/podmesh-meshfs`](https://github.com/xavdp-pro/podmesh-meshfs) — Distributed & P2P 4 MB CAS storage engine (`meshfs` + `meshfs-p2p`).
  - [`xavdp-pro/podmesh-pmbs`](https://github.com/xavdp-pro/podmesh-pmbs) — PodMesh Backup Server (`pmbs`) with 4 MB CAS deduplication and Op 10 boot proofs.
  - [`xavdp-pro/podmesh-pve-plugin`](https://github.com/xavdp-pro/podmesh-pve-plugin) — Native Proxmox VE Perl storage plugin (`MeshFSPlugin.pm`).
  - [`xavdp-pro/podmesh-cockpit`](https://github.com/xavdp-pro/podmesh-cockpit) — Interactive 2D/3D Fractal Cathedral & Cockpit Web UI.
  - [`xavdp-pro/podmesh-docs`](https://github.com/xavdp-pro/podmesh-docs) — Bilingual documentation portal and interactive validation showcase.
  - [`xavdp-pro/podmesh-installer`](https://github.com/xavdp-pro/podmesh-installer) — Unified `.deb` packaging, offline closures, and dual-channel APT repository tooling.
- `xavdp-pro/podmesh` (`v2.3.0`) remains the **full-power sovereign orchestrator** (retaining the complete workspace build for integrated local/CI verification while composing cleanly with the standalone `podmesh-*` modules). See [`MODULES.md`](MODULES.md).
