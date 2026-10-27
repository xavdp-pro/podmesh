# Modules & Ecosystem Composition — podmesh (v2.3.0)

In **v2.3.0** (**SHAPER PHOENIX Modular Ecosystem Split**), `podmesh` serves as the sovereign container and universe orchestrator at the center of a modular suite of 7 public repositories and 7 private `-lab` companion workshops.

Each module can be deployed and updated independently on Linux / Proxmox VE hosts or composed into the complete PodMesh sovereign stack.

---

## 1. The 7 Canonical Public Repositories & Private Lab Companions

| Component | Public Product Repository | Private Lab Companion | Version | Role & Scope |
| :--- | :--- | :--- | :---: | :--- |
| **PodMesh Core** | [`xavdp-pro/podmesh`](https://github.com/xavdp-pro/podmesh) | [`xavdp-pro/podmesh-lab`](https://github.com/xavdp-pro/podmesh-lab) | `v2.3.0` | Sovereign Podman orchestrator, `podmesh-node` & `podmesh-manager` MariaDB units, Ed25519 epoch fencing, and `podmesh-ctl` CLI. |
| **MeshFS** | [`xavdp-pro/podmesh-meshfs`](https://github.com/xavdp-pro/podmesh-meshfs) | [`xavdp-pro/podmesh-meshfs-lab`](https://github.com/xavdp-pro/podmesh-meshfs-lab) | `v2.3.0` | 4 MB SHA-256 Content-Addressable Storage (CAS) distributed & P2P filesystem (`meshfs` Rust daemon + `meshfs-p2p` engine). |
| **PMBS** | [`xavdp-pro/podmesh-pmbs`](https://github.com/xavdp-pro/podmesh-pmbs) | [`xavdp-pro/podmesh-pmbs-lab`](https://github.com/xavdp-pro/podmesh-pmbs-lab) | `v2.3.0` | PodMesh Backup Server (`pmbs`) in Pure Rust: 4 MB CAS deduplication, immutable snapshots, cross-site sync, and Op 10 boot proofs. |
| **PVE Plugin** | [`xavdp-pro/podmesh-pve-plugin`](https://github.com/xavdp-pro/podmesh-pve-plugin) | [`xavdp-pro/podmesh-pve-plugin-lab`](https://github.com/xavdp-pro/podmesh-pve-plugin-lab) | `v2.3.0` | Native Proxmox VE storage plugin (`PVE::Storage::Custom::MeshFSPlugin`) for QEMU/KVM disks, LXC rootdirs, and VZDump backups. |
| **Cockpit UI** | [`xavdp-pro/podmesh-cockpit`](https://github.com/xavdp-pro/podmesh-cockpit) | [`xavdp-pro/podmesh-cockpit-lab`](https://github.com/xavdp-pro/podmesh-cockpit-lab) | `v2.3.0` | Interactive 2D/3D Fractal Cathedral & Sovereign Cockpit UI for cluster supervision and human–AI tandem operations. |
| **Docs & Showcase** | [`xavdp-pro/podmesh-docs`](https://github.com/xavdp-pro/podmesh-docs) | [`xavdp-pro/podmesh-docs-lab`](https://github.com/xavdp-pro/podmesh-docs-lab) | `v2.3.0` | Official multi-language documentation portal, Ideal Scene 34/34 qualification explorer, and interactive architecture showcase. |
| **Installer & APT** | [`xavdp-pro/podmesh-installer`](https://github.com/xavdp-pro/podmesh-installer) | [`xavdp-pro/podmesh-installer-lab`](https://github.com/xavdp-pro/podmesh-installer-lab) | `v2.3.0` | Unified Debian (`.deb`) package builders, air-gapped closures, and dual-channel APT repository generators (`trixie-dev` / `trixie-stable`). |

---

## 2. How the Modules Compose with `podmesh`

```text
                        ┌───────────────────────────────────────┐
                        │            podmesh-cockpit            │
                        │    (2D/3D Fractal Cockpit & Web UI)   │
                        └───────────────────┬───────────────────┘
                                            │ Pure AJAX / JSON API
                                            ▼
┌───────────────────────┐       ┌───────────────────────┐       ┌───────────────────────┐
│    podmesh-meshfs     │◄─────►│        podmesh        │◄─────►│     podmesh-pmbs      │
│  (:7443 / meshfs.sock)│       │  (Core Node/Manager   │       │  (:8007 / pmbs.sock)  │
│  4 MB CAS & P2P Store │       │   & podmesh-ctl CLI)  │       │  CAS Dedup & Op 10    │
└───────────▲───────────┘       └───────────▲───────────┘       └───────────────────────┘
            │                               │
            │ Storage Hooks                 │ Packaging & APT Delivery
┌───────────┴───────────┐       ┌───────────┴───────────┐       ┌───────────────────────┐
│  podmesh-pve-plugin   │       │   podmesh-installer   │       │     podmesh-docs      │
│   (MeshFSPlugin.pm)   │       │ (.deb & APT Channels) │       │ (34/34 Iron Showcase) │
└───────────────────────┘       └───────────────────────┘       └───────────────────────┘
```

### Interface Contracts Across Modules
1. **Control & Orchestration (`podmesh` ↔ `podmesh-meshfs` / `podmesh-pmbs`)**:
   - `podmesh` and `podmesh-ctl` orchestrate universe lifecycles, HA epoch rotations, and cross-node migrations while delegating volume snapshots and CAS chunk transfers to `podmesh-meshfs` (`/var/lib/meshfs/meshfs.sock` or `127.0.0.1:7443`) and `podmesh-pmbs` (`/var/lib/pmbs/pmbs.sock` or `127.0.0.1:8007`).
   - Because both `podmesh-meshfs` and `podmesh-pmbs` share the **4 MB SHA-256 CAS chunk invariance law**, snapshots taken in MeshFS transfer to PMBS without re-chunking or re-hashing.
2. **Hypervisor Integration (`podmesh-pve-plugin` ↔ `podmesh-meshfs`)**:
   - `podmesh-pve-plugin` installs `/usr/share/perl5/PVE/Storage/Custom/MeshFSPlugin.pm` on Proxmox VE nodes and communicates with the local `podmesh-meshfs` daemon to provision, snapshot, clone, and migrate VM/LXC volumes.
3. **Human–AI Surfaces (`podmesh-cockpit` & `podmesh-docs` ↔ `podmesh`)**:
   - `podmesh-cockpit` provides the live operational UI obeying the four permanent Web Surface rules (Pure AJAX `fetch`, custom modals, styled pickers, searchable lists).
   - `podmesh-docs` publishes the architecture specifications, glossary bindings, and 34/34 physical iron qualification proofs.
4. **Distribution & Packaging (`podmesh-installer` ↔ All Modules)**:
   - `podmesh-installer` compiles and packages the standalone `.deb` artifacts (`podmesh`, `podmesh-cli`, `podmesh-meshfs`, `podmesh-backup-server`, `podmesh-pve-plugin`) and indexes them into the `trixie-dev` and `trixie-stable` APT repositories.

---

## 3. Optional Git Submodule Composition

Operators or CI pipelines wishing to assemble the decoupled repositories side-by-side under a unified workspace tree can attach the standalone modules as Git submodules:

```ini
[submodule "modules/podmesh-meshfs"]
	path = modules/podmesh-meshfs
	url = https://github.com/xavdp-pro/podmesh-meshfs.git
[submodule "modules/podmesh-pmbs"]
	path = modules/podmesh-pmbs
	url = https://github.com/xavdp-pro/podmesh-pmbs.git
[submodule "modules/podmesh-pve-plugin"]
	path = modules/podmesh-pve-plugin
	url = https://github.com/xavdp-pro/podmesh-pve-plugin.git
[submodule "modules/podmesh-cockpit"]
	path = modules/podmesh-cockpit
	url = https://github.com/xavdp-pro/podmesh-cockpit.git
[submodule "modules/podmesh-docs"]
	path = modules/podmesh-docs
	url = https://github.com/xavdp-pro/podmesh-docs.git
[submodule "modules/podmesh-installer"]
	path = modules/podmesh-installer
	url = https://github.com/xavdp-pro/podmesh-installer.git
```

Meanwhile, `xavdp-pro/podmesh` continues to maintain its internal Cargo workspace members (`crates/meshfs`, `crates/pmbs`, `crates/podmesh-cli`) at `v2.3.0` so that `cargo check --workspace` and `cargo test --workspace` remain 100% self-contained.
