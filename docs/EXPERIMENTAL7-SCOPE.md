# Experimental 7 — frozen increment scope

Updated: 2026-09-15. Owner: Xavier de Poorter.

This document freezes what the next signed Debian package (`0.1.0~experimental7`)
is allowed to claim. It exists so lab evidence can become a demonstrable stack without
inflating the product story.

## What experimental7 ships (coded on `main`, target qualification)

Built from `main` at or after the agent-cut harness fix (multiline nft ruleset,
`nft -c`, apply via `systemd-run` for partition suites).

| Area | In the binary / package | Lab evidence (2026-09-15) |
| --- | --- | --- |
| Lifecycle, clone, journal, CLI | Yes (continues experimental4→6 line) | Installed experimental6 on three hosts; suites unchanged |
| Serial migration M1–M3, collector M4 on `main` | Yes | Prior lab + `check-migration-collector` on `main` line |
| Recovery points M5 | Yes | Two-host suites; not yet in a published package |
| Activation, epochs, permits, fence, fence preview | Yes | Manager and HA suites on `podmesh-dev-ha` |
| Managed network, NAT matrix, effects ledger, crash safety | Yes | Three-host network suites |
| Secrets outside images, secrets crash safety | Yes | `check-secrets-image-free`, `check-secrets-crash` |
| Manager replicas, governor, service address, control | Yes | Cursor campaign `campaign-cursor-2026-09-15T2138Z` |
| Partition, partition-agent-cut, publisher, publisher-crash, publisher-agent-cut | Yes | Same campaign; agent-cut required harness fix |
| Ed25519 takeover proofs | Yes (`ed25519-dalek`, verification only) | `check-takeover-proof-signature` |
| Self-fence timer units | Shipped **disabled**; mandate required | `check-fence-timer`, agent-cut suites |

## What experimental7 does **not** claim

- **Production high availability** or a customer SLA.
- **Manager universe installed as the default `podmesh.service` workload** — M-U2 remains
  lab-qualified on isolated `podmesh-dev-ha.service` until install rehearsal is recorded.
- **Actual hypervisor VM destruction** (Codex step 5) — recovery-managed simulates stop/purge;
  VMID loss is operator-authorized separately.
- **Published manager Alpine image** — `m-u2-generic` is built locally on lab hosts.
- **Control-services universe**, Backup Server B0+, UI, join/leave occupied hosts.
- **Clean-host first install** of experimental7 — not rehearsed until after package build.
- **Collector line from `codex/collector-completion`** — experimental6 on hosts used that
  branch; experimental7 packages **`main`'s M4/M5 collector** unless Codex reverses this
  in writing (see decision D1 below).

## Gates before `deb.xavdp.pro` publishes experimental7

| ID | Gate | Owner | Status |
| --- | --- | --- | --- |
| G1 | Agent-cut harness fix committed on `main` | Cursor / Xavier | Ready to commit |
| G2 | `cargo build --release --locked` and `packaging/build-deb.sh` produce `dist/podmesh_0.1.0~experimental7_amd64.deb` | Build host | Run after G1 |
| G3 | **D1** Collector: package `main`'s collector (default) or document cherry-pick | Codex + Xavier | **OPEN** — default: `main` |
| G4 | **D2** Approve `ed25519-dalek` 2.2.0 in supply chain | Xavier | **OPEN** |
| G5 | Codex diff review `dff3e65..HEAD` (step 7) | Codex | **OPEN** |
| G6 | Install experimental7 on **one** lab host beside experimental6; rerun `check-installed` + one manager suite on `podmesh-dev-ha` | Cursor | After G2 |
| G7 | **D3** Authorize VM destruction test (candidate lab-a / VMID 9100) | Xavier | **OPEN** — not blocking G2–G6 |
| G8 | Product page + honest limitations paragraph | Xavier | **OPEN** — can draft from this file |

## Demonstration script for a VPE (15 minutes, honest)

1. Show signed APT at `deb.xavdp.pro` and `apt-cache policy podmesh` on a lab host.
2. Show three-host **`podmesh-dev-ha`** JSON PASS summary:
   `podmesh-lab/cursor/CURSOR-HANDOFF-2026-09-15T2210Z.md`.
3. Show one live operation: `podmesh` CLI identity + a refused request (epoch/lease).
4. State explicitly: experimental7 extends the **engine**; the **installed demo stack**
   for manager HA is the dev-ha service path until G6 completes.

## References

- Inventory: `docs/README.md` (reconciled 2026-09-15).
- Resumption: `docs/CURRENT-STATE.md`.
- Codex order: `podmesh-lab/claude/CODEX-REVIEW-B1-B3-CLOUDFLARE-2026-09-15.md`.
- Live reproduction: `podmesh-lab/cursor/CURSOR-HANDOFF-2026-09-15T2210Z.md`.
