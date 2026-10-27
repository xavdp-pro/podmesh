# Agents: what goes where (v2.3.0)

In **v2.3.0** (**SHAPER PHOENIX Modular Ecosystem Split**), PodMesh lives across the sovereign orchestrator pair (`podmesh` / `podmesh-lab`), six specialized `podmesh-*` modular repository pairs, and one vault. Every agent working on PodMesh (Claude Code, Codex, Cursor, Antigravity or any other) follows this split. Dialogue with the operator is French; everything written into any repository is English (`en-US`).

| Where | What belongs there | What never goes there |
| --- | --- | --- |
| **`xavdp-pro/podmesh`** (this repository, public, `v2.3.0`) | The sovereign orchestrator product: code, tests, tools, packaging; the documentation needed to understand, build, qualify and operate PodMesh; the ideal scene, its program and its certification criteria (the ideal scene is common to both repositories and lives here only); [`LAW.md`](LAW.md), [`LINEAGE.md`](LINEAGE.md), [`MODULES.md`](MODULES.md) | Laboratory status and campaign narratives, reviews, decisions and coordination notes, laboratory addresses and host names as defaults, anything private |
| **`xavdp-pro/podmesh-lab`** (private, `v2.3.0`) | The maintainers' master workshop: the current state and handoff, the status of every certification item with its evidence, campaign scripts and records, reviews, decisions, mandates, research notes, agent handoffs, laboratory configuration without secrets; it pins this repository as a submodule to read the ideal scene without copying it | Secrets and private kits, large binary evidence |
| **`xavdp-pro/podmesh-meshfs`** & **`podmesh-meshfs-lab`** | Standalone 4 MB CAS distributed & P2P storage engine (`v2.3.0`) and its private qualification workshop | Private host keys, secrets, unverified lab dumps in the public repo |
| **`xavdp-pro/podmesh-pmbs`** & **`podmesh-pmbs-lab`** | Standalone PodMesh Backup Server (`v2.3.0`) with Op 10 boot proofs and its private qualification workshop | Private host keys, secrets, unverified lab dumps in the public repo |
| **`xavdp-pro/podmesh-pve-plugin`** & **`podmesh-pve-plugin-lab`** | Standalone Proxmox VE Perl storage plugin (`MeshFSPlugin.pm`, `v2.3.0`) and its private qualification workshop | Private host keys, secrets, unverified lab dumps in the public repo |
| **`xavdp-pro/podmesh-cockpit`** & **`podmesh-cockpit-lab`** | Standalone 2D/3D Fractal Cathedral & Sovereign Cockpit UI (`v2.3.0`) and its private design/qualification workshop | Private host keys, secrets, unverified lab dumps in the public repo |
| **`xavdp-pro/podmesh-docs`** & **`podmesh-docs-lab`** | Standalone multi-language documentation portal and 34/34 iron qualification showcase (`v2.3.0`) and its private workshop | Private host keys, secrets, unverified lab dumps in the public repo |
| **`xavdp-pro/podmesh-installer`** & **`podmesh-installer-lab`** | Standalone `.deb` package builders, offline closures, and dual-channel APT repository tooling (`v2.3.0`) and its private workshop | Private signing keys, secrets, unverified lab dumps in the public repo |
| **The vault** (or an encrypted backup the operator names) | Signing keys, tokens, credentials, private replica kits | — |

Rules:
1. A document that tells an adopter what PodMesh is, promises or how to operate it goes here. A document that tells the
   maintainers where the work stands, what was measured on their laboratory, or who decided what goes to `podmesh-lab`.
2. The ideal scene, the program and the certification criteria are edited here only. Their status (which item is met,
   with which evidence) is kept in `podmesh-lab/status/`.
3. Tools and tests take their hosts, addresses and workstation paths from the environment; no laboratory address, host
   name or workstation path is ever a default.
4. Nothing durable is kept only in a temporary directory.

## Current cross-project documentary route

Resolve the tandem's task through [ecosystem/component ownership](docs/ECOSYSTEM-AND-COMPONENT-OWNERSHIP.md),
the [glossary](https://github.com/xavdp-pro/shaper-glossary), required interfaces
and exact implementation owner. Use component in current concepts while retaining
runtime/storage/API IDs. Durable application state follows private MariaDB
ownership and partitions chosen for actual workload/retention need. The existing
adoption record below retains its exact scope/version; source alignment does not
silently re-adopt a framework or activate a running system.

## Adopted development standard

The human–agent tandem has explicitly adopted SHAPER OS as the governing
development standard for this project's declared scope. Treat its applicable
obligations as binding requirements, not optional background context.

- Corpus location: https://github.com/xavdp-pro/SHAPER-OS-V1.15
- Immutable revision: `173ae591988e1f71a33f65289fc26b9de4aaf3d7` (`main`, resolved 2026-10-07; the corpus read for this adoption was that commit, clean detached worktree)
- Adoption scope: ongoing development of the PodMesh product in this repository and of the maintainers' workshop in `xavdp-pro/podmesh-lab`. Covers design, implementation, documentation, tests and evidence for the local node, the replicated manager and laboratory qualification. Does not convert PodMesh into a SHAPER universe (Vault/Logger/Queue/Maestro) or rebuild the laboratory. Node and manager private-MariaDB delivery proceeds in bounded committed slices; an old assembly branch or a workstation gate is not the delivery authority.
- Declared architectural roles and applicable profiles: optional manager of SHAPER `nested` universes (Rule 11), without governor or maker authority; per-host PodMesh node; active manager replica whose activation depends on an externally issued lease and epoch. Standalone Podman operation remains valid. Profile `SEP22-CONTAINER-MARIADB` applies. The durable roles **`podmesh-node`** and **`podmesh-manager`** are declared functional units under that profile (Rules 4 and 26): one responsibility, isolated Podman boundary, private MariaDB **server instance** each, slug = system user = DB user = database. See `docs/FUNCTIONAL-UNITS.md`.
- Architecture context: `xavdp-pro/shaper-three-layers` at `a8eddf95b30a5952267990928e65c5e9c61131f8`. Its layer and authority distinctions supplement, never replace, the adopted V1.15 obligations. Host privileges are delegated execution capabilities, not governance authority.
- Production path: `docs/SIMPLE-PRODUCTION-PATH.md` — **A** container life, then **B** move + replicate, **C** later. Commit source before building; carry that immutable pin, artifact manifest and hashes through runtime qualification. Build/test hosts are explicit deployment inputs, never an operator workstation prerequisite.
- Candidate source interfaces: `packaging/podmesh-node/private-unit`, `src/host_adapter.rs`, `docs/HOST-ADAPTER-CONTRACT.md` and `packaging/podmesh-manager/private-unit`. Recipes/provider do not establish a complete installer or runtime qualification. Private app launches suppress inherited image volumes; only declared owned app/DB volumes are permitted. Node product proof driver/scenario: `packaging/podmesh-node/qualification`; host/VM/recovery execution belongs to its separately authorized runtime owner.
- Application identity and private store stay inside each declared unit boundary. A privileged host adapter is a separate, scoped capability provider; it does not become the application account, own its database, or acquire Governor/Maker authority. See `docs/FUNCTIONAL-UNITS.md`.
- Current qualification and conformity observations are maintained in the distinct private `podmesh-lab/status/` source. Product intentions, operating contracts and acceptance criteria remain here; current campaign results retain their exact source/artifact scope in that companion.
- Outside this mandate: arming vote keys, closing laboratory gates, production HA promotion, reference universe, editing the pinned SHAPER corpus.

Follow the corpus's AGENTS.md, LAW.md, governing map and reading contract.
Read the required foundation and applicable detailed contracts before work.

Continue ordinary development covered by the current mandate. Preserve
existing state and unaffected contracts. Apply construction or migration
procedures only when the authorized task requires them.

A declared gap does not waive an applicable obligation. Do not claim full
universe or unit conformity from adoption of the development standard alone.
Report the actual scope, changes, evidence and remaining gaps.
