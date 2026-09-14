# Campaign 6: the repeatable evidence-v3 capture

Status: **G2 PASS on real evidence v3, from a single-command driver, with derivation from the
preserved stores reproduced on all three hosts.** Three replicas of the frozen candidate
`0.1.0~manager2+gff77b1f946e8` (binary `cbd5020a…3660128`), one owned observation each, typed
graceful shutdown on all three, every raw store preserved at rest.

As every campaign before it, this qualifies **exchange accounting** and nothing else: not HA,
fencing, DNS takeover, automatic failover, production use or long-running viability. The
comparator's own output says `ha_claim: "absent"`.

## What Codex asked for on 2026-09-14, and what was delivered

> Prepare a repeatable real three-host G2 evidence-v3 capture. It must preserve the raw stores,
> inspection output, comparator input and comparator result. The campaign must be able to fail
> honestly.

`packaging/podmesh-manager/qualification/activation/campaign/run-campaign.sh campaign` runs
the whole sequence from the workstation and stops at the first failure, rolling back what it
activated. Per host it preserves the four stage files with sidecars, the activation ledger,
the raw store copied at rest (`preserved-store/manager.sqlite`, 8.6–11.3 MB, with
`SHA256SUMS`) and an inspection derived from that copy; for the campaign it preserves the
comparator's input (the twelve stage files), its result, the derivation result and a public
summary. The public part is under `docs/qualification/manager2-g2-live-6/`; the stores and
the derived inspections carry raw identifiers and stay private on the workstation.

**And it did fail, honestly, once.** The first `preserve lab-a` was refused by the candidate:
*"database must be a direct child of the declared state directory"*. Inspecting a copy needs a
configuration rebuilt with only the database path changed — the trap work item 7 had already
met and this driver had not learned from. The live phases were all complete and every host
was already rolled back, so the driver was corrected and `finish` ran the preservation on
stores at rest, unchanged since the shutdown. The summary lists that attempt under
`driver_incidents`, with its exit code, and the verdict is computed over the live phases, the
comparator and the derivation — a driver's defect is reported, never counted against the
candidate and never hidden.

## What the gate returned

| | |
| --- | --- |
| canonical convergence evidenced | **true** (three equal digests `079784c0…`, history 15 → 18) |
| pre-existing incomplete attempts (debt from campaigns 4 and 5) | 211, retained and reported |
| new incomplete attempts | 103 |
| **accounted** | **103** |
| unaccounted | **0** (`join_failed` 0, `peer_has_no_record` 0) |
| terminal attempts | 237 |
| failures | 0; undecided conditions 5 (the collector-honest ones, as always); unmatched inbound rows 2, reported |
| derivation from the preserved stores | reproduced on lab-a, lab-b and lab-c: digests and counts of the copy equal the live post-cleanup projection |

Campaign 5 had left 3 unaccounted strands; this one leaves none. The difference is not the
candidate — it is byte-identical — but the sample: every stranded sender attempt of this run
was retried until served, and the join found its receiver rows.

## The latency defect, third data point

One observation, on lab-c, needed **114 identical resubmissions** of the sender's request
before it was observed (campaign 4: 30; campaign 5: 78). The audit tables now hold 4340,
5165 and 4819 rows. This is the curve `MANAGER-PRE-REPLY-VERIFICATION.md` measures and its
addendum of the same day discusses; a G2 PASS says nothing about it, by design, and the
number is written here so that nobody reads the PASS as viability.

## Timeline

Prepared 17:31 UTC; three activations by 17:36; observations 17:37; converged on the second
poll (five seconds); converged captures by 17:40; three typed shutdowns by 17:44; nine
minutes for the live phases. Preservation, fetch, comparison and derivation followed after the
driver repair.

## What campaign 6 does not show

The same list as campaign 5, unchanged: no takeover, no fencing, no exclusive activation, no
DNS, no host loss, no partition, no long run. Codex's next items — qualifying the replication
data path separately from activation, then a three-host takeover experiment with an external
authority — start from here and are not begun.
