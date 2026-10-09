# Agents: what goes where

PodMesh lives in two repositories and one vault. Every agent working on PodMesh (Claude Code, Codex, Cursor,
Antigravity or any other) follows this split. Dialogue with the operator is French; everything written into either
repository is English.

| Where | What belongs there | What never goes there |
| --- | --- | --- |
| **`xavdp-pro/podmesh`** (this repository, public) | The product: code, tests, tools, packaging; the documentation needed to understand, build, qualify and operate PodMesh; the ideal scene, its program and its certification criteria (the ideal scene is common to both repositories and lives here only) | Laboratory status and campaign narratives, reviews, decisions and coordination notes, laboratory addresses and host names as defaults, anything private |
| **`xavdp-pro/podmesh-lab`** (private) | The maintainers' workshop: the current state and handoff, the status of every certification item with its evidence, campaign scripts and records, reviews, decisions, mandates, research notes, agent handoffs, laboratory configuration without secrets; it pins this repository as a submodule to read the ideal scene without copying it | Secrets and private kits, large binary evidence |
| **The vault** (or an encrypted backup the operator names) | Signing keys, tokens, credentials, private replica kits | — |

Rules:
1. A document that tells an adopter what PodMesh is, promises or how to operate it goes here. A document that tells the
   maintainers where the work stands, what was measured on their laboratory, or who decided what goes to `podmesh-lab`.
2. The ideal scene, the program and the certification criteria are edited here only. Their status (which item is met,
   with which evidence) is kept in `podmesh-lab/status/`.
3. Tools and tests take their hosts, addresses and workstation paths from the environment; no laboratory address, host
   name or workstation path is ever a default.
4. Nothing durable is kept only in a temporary directory.

## Adopted development standard

The human–agent tandem has explicitly adopted SHAPER OS as the governing
development standard for this project's declared scope. Treat its applicable
obligations as binding requirements, not optional background context.

- Corpus location: https://github.com/xavdp-pro/SHAPER-OS-V1.15
- Immutable revision: `173ae591988e1f71a33f65289fc26b9de4aaf3d7` (`main`, resolved 2026-10-07; the corpus read for this adoption was that commit, clean detached worktree)
- Adoption scope: ongoing development of the PodMesh product in this repository and of the maintainers' workshop in `xavdp-pro/podmesh-lab`. Covers design, implementation, documentation, tests and evidence for the local node, the replicated manager and laboratory qualification. Does not convert PodMesh into a SHAPER universe (Vault/Logger/Queue/Maestro) or rebuild the laboratory. Node-store MariaDB cutover is authorized on `cursor/assembly-2026-10-07`; manager-store MariaDB waits for the node.
- Declared architectural roles and applicable profiles: optional manager of SHAPER `nested` universes (Rule 11), without governor or maker authority; per-host PodMesh node; active manager replica whose activation depends on an externally issued lease and epoch. Standalone Podman operation remains valid. Profile `SEP22-CONTAINER-MARIADB` applies. The durable roles **`podmesh-node`** and **`podmesh-manager`** are declared functional units under that profile (Rules 4 and 26): one responsibility, isolated Podman boundary, private MariaDB **server instance** each, slug = system user = DB user = database. See `docs/FUNCTIONAL-UNITS.md`.
- Production path: `docs/SIMPLE-PRODUCTION-PATH.md` — **A** container life, then **B** move + replicate, **C** later. Assembly branch: `cursor/assembly-2026-10-07`.
- Existing conformity gaps (do not expand into campaigns from this list):
  - Supervision adapter not qualified (`docs/SHAPER-SUPERVISION.md`).
  - Published package ≠ qualified package (`ACCEPTANCE-TEST-PLAN.md` note 2026-09-18).
  - Nested move: verify under PodMesh (outer Podman moves; nested follows); then align site and wiki.
  - `INTENT.md`: still missing Rule 0B / Rule 7 headers — add on next edit of that file.
  - Epoch gate and guardian still on a workstation (ideal-scene departure 2 / A5) — separate mandate.
  - Gate 4 HELD; no vote keys from this assembly.
  - Node MariaDB (`podmesh-node`) in progress on the assembly branch; manager MariaDB after; one server instance per slug.
- Outside this mandate: arming vote keys, closing laboratory gates, production HA promotion, reference universe, editing the pinned SHAPER corpus.

Follow the corpus's AGENTS.md, LAW.md, governing map and reading contract.
Read the required foundation and applicable detailed contracts before work.

Continue ordinary development covered by the current mandate. Preserve
existing state and unaffected contracts. Apply construction or migration
procedures only when the authorized task requires them.

A declared gap does not waive an applicable obligation. Do not claim full
universe or unit conformity from adoption of the development standard alone.
Report the actual scope, changes, evidence and remaining gaps.
