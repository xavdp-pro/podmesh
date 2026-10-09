# Public claims — path B (move and replicate)

Date: 2026-10-07. Branch: `cursor/assembly-2026-10-07`. Audience: landing page,
staff wiki, and any summary that must match laboratory evidence for **G3** and **G4**
only (see `docs/SIMPLE-PRODUCTION-PATH.md` § B). Path **A** (container life) and
path **C** features are out of scope here.

Maintainers hold typed reports and logs in the private workshop; this document is the
sanitized wording adopters and the public site should use.

## Move (one proof under the PodMesh contract)

**Claim (allowed):** On the assembly development tree, maintainers recorded **one**
verified **nested** outer-universe move between two laboratory hosts: checkpoint on
the source, carry of artifacts, destination restore, `complete_transfer`, and
`retire_source`, with stable universe UUID and a quiesced inner counter continuity
check after restore.

**Limits (required in the same breath):**

- Fixture: `migration-lab:extended` nested outer with inner Podman counter workload;
  not arbitrary volumes, networking, or production packaging.
- Inner counter continuity is against a **quiesced disk/log baseline**, not
  memory-continuous capture across the move unless separately proven.
- Prior vzcriu kit experiments remain **kit-scoped** evidence; they are not this
  `podmeshd` typed move.
- APT `0.1.0~experimental7` on hosts is **not** automatically qualified for this
  chain; evidence pins specific `podmeshd` builds from the assembly branch.

**Do not claim:** general fault tolerance, Gate 4 closure, vote keys, or that every
migration profile is production-ready.

## Replicate (one proof under the PodMesh contract)

**Claim (allowed):** On the same assembly line, maintainers recorded **one**
verified two-host recovery-point campaign: `replicate-universe.py` configure → run →
`takeover --planned`, **PASS** for both **stopped** and **live** capture between the
same laboratory pair.

**Limits (required):**

- P6 guardian / full automated HA beyond this harness is **not** claimed.
- Scope matches the replicate field PLAN P5 bar; not a third host, not managed
  network qualification, not manager MariaDB.

**Do not claim:** uninterrupted application availability for all workloads; live capture
requires workloads that handle signals honestly (documented harness constraint).

## What stays “not yet validated through the service”

Until path **A** closes and package qualification catches up, the experimental Debian
package blurb may still say migration and HA are **not** validated **through the
published package acceptance gates**. Path B proofs above are **development-tree,
laboratory-contract** evidence — align narrative by citing both the proof and the
package/acceptance gap (`docs/ACCEPTANCE-TEST-PLAN.md`, dated notes).

## Site and wiki alignment checklist

| Surface | Must mention move claim | Must mention replicate claim | Must state limits |
| --- | --- | --- | --- |
| deb.xavdp.pro PodMesh card | yes | yes | yes |
| Staff wiki PodMesh pages | yes | yes | yes |
| This repository `README.md` “Current scope” | when edited for B | when edited for B | yes |

Update external site/wiki repositories separately; keep them identical to this file’s
claims and limits.
