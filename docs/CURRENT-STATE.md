# PodMesh — current state and resumption guide

Updated: 2026-09-15. Owner: Xavier de Poorter, collaborating with OpenAI Codex, Claude, and Cursor.

This is a handoff, not a replacement for INTENT.md or the detailed contracts.
Read this file first, then the relevant checklist item and its evidence. Do not
re-read the entire conversation or repeat passed tests without a reason.

**Do not keep PodMesh work only under `/tmp`.** A reboot on this workstation
wipes `/tmp`. Durable store: `/home/zaza/Bureau/REMOTE3/podmesh-lab/`
(see `/home/zaza/Bureau/REMOTE3/WORKSTATION-STORE.md`). Old `/tmp/podmesh-web`
and `/tmp/podmesh-claude` names are archive or reboot-volatile aliases.

## Product direction and decisions

PodMesh serves human–agent tandems. API and CLI come first; a human interface
will use the same operations. Hosts remain autonomous Linux machines; the lab's
hypervisor is not a product dependency. Support standalone deployment and optional
ShaperOS integration, preferred internally for existing supervision and logs.
The governor owns intent and the registry; one maker per host orchestrates local
PodMesh mechanics. The intended demonstration is SaaS -> manager -> shop child
universes spread across three hosts, with an operational assessment.

Stable UUIDs identify hosts and universes; human-readable names are separate.
Joining occupied hosts and reconnecting must not replace existing identities.
The proposed logical network is /16 with /24 allocation pools per host; preserve
universe IPs during movement and update routes. WireGuard is optional for control,
workload traffic and copies. Shared DNS, image availability and bootstrap remain
future work. No actual prefix is selected here.

During partitions, local work is limited to preassigned non-overlapping scopes.
A lower ID can select a coordinator after reconciliation, not decide which data
is true. A replica is not permission to activate a second exclusive workload.
Manager HA, safe takeover and memory/disk recovery remain to design and prove.

Debian packages target hosts. Alpine is preferred for suitable container tests.
The possible ShaperOS/Vox port to Alpine is explicitly outside this project.
Backup Server must support standalone Linux or containers/VMs/VPS, with ShaperOS
optional. Storage experiments will reuse one dedicated 60 GB disk per lab host:
LVM2/thin, then ZFS, then Btrfs; preserve evidence before resetting only those
new disposable disks. These disks have not been added.

Dialogue is French; repository deliverables are English. Public reusable sources,
private infrastructure configuration, secrets outside all Git repositories.
Credit Xavier de Poorter and the contributing agents, preserving upstream credit.

## Published and checked

- PodMesh source baseline: `c96655b`, public `xavdp-pro/podmesh` (`8d1d131` source-side
  checkpoint milestone, `c96655b` provisional migration protocol, both pushed 2026-09-11).
  Installed packages still carry the experimental4 scope below.
- `0.1.0~experimental4` installed through signed APT on all three existing lab hosts.
- Six passing suites per host: installation 7 checks, lifecycle 6, deletion
  ownership 13, start/stop 40, cloning 25, interrupted cloning 9.
- Removal, reinstall, rollback to experimental3 and re-upgrade passed on one host;
  identity, journal and a running workload survived. Not a clean-host test.
- Independent Claude Sonnet 5 review found no blocking defect in this scope;
  Codex verified evidence and the public package hash. See REVIEW-EXPERIMENTAL4.md.
- Runtime `podmesh-vzcriu` and controller/node helper packages published and
  installed on three hosts. Helper dispatch tests: 30 passed. Fork commits
  `bcfb270` and `a753ead`. This does not prove migration through PodMesh.
- Signed APT repository: https://deb.xavdp.pro/. Infrastructure source is private.
- Earlier nested-counter migrations belong to the separate VFS reproduction kit,
  not the default-store API. Keep their evidence and scope separate.

## New development, not yet published or independently accepted

Claude Code Opus 5, medium effort, completed a source-side checkpoint milestone.
The working tree adds `migration_preflight`, `migration_checkpoint` and
`migration_status`, plus `src/migration.rs` and two test scripts. It uses the
normal rootful overlay store with network-disabled, mount-free journal-owned
fixtures. Source reservations block conflicting generic lifecycle operations.

Reported development tests on one host: source checkpoint 33 checks and
interruption 8 checks, with lifecycle/deletion tests rerun and earlier suites on
the same binary. Root has read the report but has not independently reviewed this
new code or accepted the milestone. No new package or installed-service upgrade
was made. The development service is stopped; disposable containers were removed;
archives, reservations and diagnostics remain in the isolated development state.

A checkpoint stops its source. No transfer or destination restore exists in this
milestone, and no default-store archive has been restored. Restorability and
memory continuity are therefore unproven. A local reservation is not fencing:
direct administration and older package versions can bypass it. No release API
exists. Do not enable this unfinished workflow for ordinary workloads.

## Review of the checkpoint milestone

2026-09-11, Claude Code Opus 5. Same model family as the implementer, so not an
independent review; see REVIEW-MIGRATION-SOURCE.md. Six defects corrected:
checkpoint by container ID, 1 GiB memory bound, space check in container storage,
scope completion state, reuse of an empty artifact directory, stop wording. Rebuilt
and rerun on the same development host and isolated service: source checkpoint 35,
interruption 8, lifecycle 6, deletion 13, start/stop 40, clone 25, interrupted
clone 9; no leftovers; development service stopped. The pre-review tree is backed
up in evidence/review-migration-source/. An independent counter-review by Claude
Sonnet 5 found no blocking defect; its two should-fix items and notes were applied,
the seven suites rerun with the same counts, and the milestone committed. Not packaged.
This file itself stays untracked: it carries internal cost and delegation notes.

## Destination milestone (lot M2), 2026-09-12

A universe now migrates through the local API from one lab host to another and back,
keeping its memory: transfer authorization, destination preflight, restore, completion,
source retirement and abort, with ownership imported on the destination. Evidence on two
isolated development services: destination 59 checks, interrupted restore 26, and the seven
earlier suites rerun on the same binary. Memory continuity was observed from outside the
universe (same memory-only token, counter continuing, never restarting at zero).

Two defects found and fixed: unbounded reads of runtime logs (a damaged archive made CRIU
write a 6.5 GB log and the service was killed for memory) and `INVOCATION_ID` surviving
inside the transient scope. One defect reported and not fixed: a restore that fails after
Podman started leaves conmon and CRIU processes PodMesh did not start, still writing; the
abort reports them and kills nothing. Independent counter-review (Claude Sonnet 5): commit
as is, no blocking defect; its documentation corrections are applied.

## Garbage collection (lot M4), 2026-09-12 — implemented, corrected, not published

Against the operator's contract GARBAGE-COLLECTION.md, untouched by the lot. A read-only
host-wide `garbage_collect_plan` and a separately authorized `garbage_collect_apply` that
names the plan it applies, repeats every proof immediately before each effect and verifies
each result from outside. Terminal reservation classes 1 and 2 become a `collected`
reservation plus a tombstone that refuses a blind `create` of that UUID for good while
leaving the source operable; a failed restore claim is ended by calling the existing
`migration_restore_abort`, with no process management duplicated. No timer, no scheduler.

Counter-reviewed mid-flight by Codex (independent-review-codex.md), verdict OPEN with four
findings: a committed effect reportable as a refusal, a second collection of one identity
blocked by a unique key, a reclaim budget counted after the delegated call, and a class 3
observation never turned into a verdict. All four fixed with their own tests; twelve suites
rerun on the corrected binary, all exit 0 (collector 79, recovery 36, cleanup 23,
destination 66, destination-interrupt 26, source 35, interrupt 8, lifecycle 6,
delete-ownership 13, start-stop 40, clone 25, clone-interrupt 9), plus nine unit tests.

Frozen revision: baseline `a804f0b` with uncommitted changes; `src/collector.rs`
`7c9958a7…`, `src/migration.rs` `7a387198…`, the suite `8084709c…`, the binary
`be3558db…` identical on both hosts. Independently verified here: the three source hashes
against the tree, the twelve exit statuses, the journal tables on .156 (99 runs, 18 effects,
39 tombstones, 18 history occurrences — the gap is pre-fix iterations, stated in the report),
`outside_api_windows: []` on both hosts, and `cargo test` on this workstation.

The operator withholds publication until Codex closes its review. Request prepared:
`/tmp/podmesh-claude/POUR-CODEX-2026-09-12-revue-collecteur-corrige.md`. Do not commit,
package or edit the tree while that review is in flight.

The lot proposes four wordings for GARBAGE-COLLECTION.md, which only the operator may
apply: widen class 2's second condition to "no live authorization"; let the plan read
Podman and write its own audit trail while forbidding domain and runtime mutation; say
which gate class 1's safe effect lifts and which it never lifts; and state whether an
evidence hold also blocks a terminal reservation transition and a runtime reclaim.

## Manager2 G2 — the gate is testing a reliability bar, not a correctness one (2026-09-13, settled)

Read `/tmp/podmesh-web/docs/FINDING-MANAGER2-LATE-REPLY-STRANDS.md` first; the section below
records how we got there and is superseded on the question of *why* attempts strand.

The transition to `incoming_workers: 2` was applied, compared PASS on three hosts and sealed. A
measurement run (published as such, `docs/qualification/manager2-g2-measurement/`) then settled by
measurement what two readings of the source had only argued: the accept-and-drop is gone
(`rejected_connections: 0`, strands 118 → 9), and each of the nine remaining was received,
durably imported and **replied to** by its peer — the reply simply not read inside the sender's
2 s deadline while the receiver ran seven to eight whole-audit-table re-verifications and three
fsyncing transactions. The outbound ledger closes exactly and no fact was lost.

The decisive reading is of `docs/MANAGER-G2-DURABLE-EXCHANGE.md`: its normative failure-semantics
table at `:470` and invariant G2-I07 at `:80` **require** a source to retain a prepared/incomplete
attempt when a reply is lost after the destination commits. The candidate obeys its contract. So
the gate's `incomplete_attempt_count == 0` asks that a specified failure mode never occur during a
campaign — a specification question for the gate's author, not a defect. The genuine defect is
separate: the unbounded per-insert `verify_all` (bench-measured linear, ~0.93 s for the pre-reply
sequence at 3,341 rows), which turns an anticipated rare failure into a frequent one and will hurt
a long-lived manager far more than a seven-minute campaign. It is worth fixing regardless of G2.

Two workflows carried this: 91 agents on whether the transition could ever suffice (it cannot —
the inherited floor), then 77 on the measured diagnosis, which refuted six of my own claims. I
published a wrong diagnosis twice today and both times only measurement corrected it; the lesson
is in [[methode-mesurer-avant-de-publier]]. Commits `991987b` and `9d814b2`, pushed, PR 2
commented. No store erased, no gate or captured file altered, no checkbox ticked.

## Manager2 G2 campaign 6 (2026-09-14) — G2 PASS on real evidence v3, from a repeatable driver

Codex's first engineering item of the day. The web tree now carries
`packaging/podmesh-manager/qualification/activation/campaign/run-campaign.sh` (one command,
stops at the first failure, rolls back, preserves raw stores at rest with a derived
inspection, public summary with driver incidents listed) and `docs/G2-CAMPAIGN-6-MEASUREMENT.md`
with the public evidence under `docs/qualification/manager2-g2-live-6/`. Result: convergence
evidenced, 103 new incomplete attempts all accounted, 0 unaccounted, 211 pre-existing
retained, derivation reproduced on all three hosts; one observation needed 114 resubmissions
(latency defect, third data point). Private campaign copy: `.podmesh-builds/manager2-g2-campaigns/campaign-6/`.
Lab-tested; nothing deployed; `ha_claim` absent. Codex's item 2 is done the same evening:
`docs/MANAGER-REPLICATION-DATA-PATH.md` (web tree) states, from the three preserved stores,
what converges (the fact set: 18/18/18 identical, one digest, one view), what is immutable
but per host (receipts, audit, open attempts — hence the cross-host join), and what is still
an exclusive decision (absent from the campaign, unreachable from the resident); derived by
`campaign/replication-path.py`, published as `replication-path.json`. Item 3: designed in
the web tree's `docs/MANAGER-TAKEOVER-EXPERIMENT.md` — the five requirements mapped to what
exists; the manager half cannot be tested because the resident exposes no permit path
(`status`, `shutdown`, `append_observation` only) and the campaign carries no exclusive
fact; two ways forward, both Codex's (expose a permit path in a new candidate, or run each
replica as a PodMesh universe under the epoch gate — recommended). The universe half is
exercised on three lab hosts: `tools/ha-standby.py` now serves one or more standbys, and
`tests/check-ha-three-hosts.py` plays HA-10's shape (takeover to one standby, the other
informed and refusing a stale permit, the old active superseded and rejoining as a standby).
Codex reviewed the diff the same evening (`/tmp/podmesh-claude/CODEX-REVIEW-H5-H10-M5-G2-2026-09-14.md`):
GO for the next laboratory candidate, no GO for publication or a production HA claim; and
decided the manager takeover: the manager is one logical universe under PodMesh's HA
mechanism and the epoch gate, no manager-specific election. Candidate M-U1 was built and run
the same night: `packaging/podmesh-manager/universe/` (web tree) and
`tests/check-manager-universe-ha.py` (here) — the packaged resident inside a universe with no
network, its stop translated into the typed shutdown by the entrypoint, boot facts appended
from inside, the store copied up before the resident opens it (the overlay's copy-up otherwise
trips the resident's store-identity preflight); on three lab hosts the frozen candidate's
inspection showed the store follow the universe through capture, restore, promotion and
restart (2 → 3 → 4 chained facts, integrity ok). Codex reviewed M-U1 the same night
(`/tmp/podmesh-claude/CODEX-REVIEW-M-U1-2026-09-14.md`): GO for the narrow claim, NO-GO for
packaging until four corrections — all made and rerun on the three hosts: the binary inside
the universe is attested byte-equal to the inspector before any inspection; a boot fact that is
not observed makes the start fail (exit 2, "not running when observed") and a typed shutdown
that is not acknowledged makes the stop an honest failure (exit 3, no escalation) that the
capture cycle refuses to capture after; the configuration is labelled a single-replica
portability fixture; and Alpine was tried first, with the failed proof and the exact
dependency recorded beside the Debian image (see the web tree's
`packaging/podmesh-manager/universe/ALPINE-PROOF.md`). The operator's image policy
(`/tmp/podmesh-claude/IMAGE-POLICY-UNIVERSES-2026-09-14.md`: Alpine root by default, Debian only
as a local documented exception) was then applied the same night: the resident was built for
musl from the frozen source commit in an Alpine Rust container on lab-a, and an Alpine root
image (60 MB) passed the identical three-host proof with that binary (`4111e487…`) attested as
both resident and inspector. Alpine is the default manager-universe image from here; Debian
is the compatibility branch for the frozen glibc candidate `cbd5020a…`. The musl binary is a
new build, not a frozen candidate: Codex freezes it or not. Still a universe contract
decision: a network (no replication inside universes without one) and a reachable control
socket.
On the operator's "go" (2026-09-14 evening) the HA recommendations became the tool's
defaults by hypothesis (`64892c4`): retention declared from every cycle (keep 3, one hour),
three quarantined copies, 20/5 lease and margin, the laboratory's gate, no timer.

## Manager2 G2 live activation (Codex's lot, taken over 2026-09-13) — converged, not passed

Codex's handoff `/tmp/podmesh-claude/PODMESH-REMAINING-WORK-2026-09-13.md` named one blocker
for the G2 milestone: the activation comparator refused an authentic fifth `semantic_limits`
field. That was the first of three. Behind it, the collector committed one identity under a
different salted label at each observation site, so no cross-host join could ever hold
(proven with the campaign salt, values never shown); both harness defects are fixed with
regression tests, reviewed GO by a fresh Opus context before shipping, and a second live
campaign on the three hosts passed every join. What remains is neither harness nor campaign:
the candidate deliberately keeps an outbound exchange "uncertain" when a peer drops the
connection at its `incoming_workers: 1` limit, three replicas synchronizing every second
collide continually, audit is immutable, and the gate requires `incomplete_attempt_count == 0`.
Second campaign: 50/46/27 after cleanup; first campaign passed that condition on two hosts
by luck (0/0/2). Both campaigns converged to one digest on all three replicas and cleaned up
through the typed path. Decision handed to the coordinator: raise `incoming_workers` through a
reviewed configuration transition and run a third campaign (recommended), or redefine the gate.

Work lives in the *other* clone, `/tmp/podmesh-web`, branch `codex/web-console`, PR 2, committed
as `bbbd9b1` (Xavier author, Fable 5.1 co-author) after two independent read-only Opus reviews
(GO before shipping the harness fix; GO with findings applied before the commit) and pushed
with a PR comment. Public evidence: `docs/qualification/manager2-g2-live/` (strict FAIL,
reproducible) and `docs/REVIEW-MANAGER2-G2-LIVE-ACTIVATION.md`. The workstation's root
filesystem (`pve-root`) was at 100 % — 813 MB free — during this lot; `/tmp` holds ~8 GB of
Rust build caches and review targets from Codex's sessions, regenerable but not mine to delete. Private campaign material: Codex's in
`/tmp/podmesh-manager2-g2-campaign/` (salt, alias map, replica files — never copied), mine in
`/tmp/podmesh-manager2-g2-campaign2/`; both campaigns' evidence and logs, without secrets,
in `/home/zaza/Bureau/REMOTE3/.podmesh-builds/manager2-g2-campaigns/`. Hosts left inactive
and disabled; `podmesh.service` untouched. During any manager campaign nothing else may
change on the three hosts, or the stability commitments break — so the experimental5
lifecycle upgrade of .156/.157 waits for a window between campaigns.

## Night of 2026-09-13 → 14, worked alone on Xavier's instruction

**Backup Server, revision 3 — `1cd1736` on main, pushed.** Codex counter-reviewed
revision 1 (OPEN, six blocking). Three of the six were my errors: calling an
uncoordinated live capture "crash-consistent" (a false guarantee headed for a manifest),
describing the host surface as read-only while requiring dumps and snapshots that write,
and giving restore no exclusion contract at all though it can mint a second active copy.
All corrected. A delegated read-only sweep of the whole canon then found clauses the
design never accounted for, one of which **contradicted it**: Rule 11 `:677-683` forbids
backing up images, because a backup containing them hides that the original may no longer
be reproducible. Rule 10 forbids stating any restore duration anywhere, even to dismiss
one. Rule 31 was missing entirely and creates a structural conflict — fractal erasure
versus immutable deduplicated chunks under a bucket lock — which makes the dedup domain
and the erasure granularity **one decision, not two**. Five decisions are Xavier's (X1–X5)
and B1 stays unfrozen until they are taken. Every canon line number was verified verbatim
before being written down.

**G2 lot, branch `codex/g2-accounted-attempts`, pushed.** Predicate amended (`96b2b9f`):
G2 passes on `unaccounted_incomplete_attempts == 0`, by attempt identity, never by count
delta, with the eight-condition receiver-side join. Evidence schema designed (`b7c29ec`,
`52db623`) after a delegated field inventory established the decisive fact: **only
condition 7 of the eight is even partially evaluable from today's sealed evidence** —
`capture-host.sh:137` folds the incomplete-attempt vector to an integer and drops the
four fields the join needs. The fix rests on a mechanism already proven: the nonce join
that diagnosed the nine strands, done on salted commitments so the comparator never sees
a nonce.

**Antigravity is a working third worker.** Quota was exhausted only on the Gemini 3.8
group; `claude-sonnet-4-6` answers immediately. Two gotchas: `agy` ignores the working
directory and needs `--add-dir`, and print mode needs `--dangerously-skip-permissions`.
Both delegated tasks came back accurate — every citation I spot-checked held — and both
were run against write-protected copies so nothing real was at risk. `opencode` also
works, free and credential-less, as the documented fallback. Rule for delegation:
`/tmp/podmesh-claude/USE-AGI-FOR-BOUNDED-WORK-2026-09-13.md`, eight-field task format,
and I review everything before it is integrated.

**The fresh review came back NO-GO, and its first finding was mine — `5325db0`.**
Revision 3 had **silently reverted commit `0cc7dd5`** (Xavier with Codex, 22:21, one hour
and forty-three minutes earlier): lot B0, the capture-adapters section and the storage
recommendation, 54 lines, and the only named source of a `crash-consistent` capture in
the design. I had written the file as a whole-file rewrite without re-reading it, and
misread the tooling's warning that it had changed. Restored, merged rather than pasted,
and recorded in the document itself. The same mistake destroyed Codex's own
`independent-review-codex-2.md` earlier the same night, which was **not** recoverable —
`evidence/` is outside git. Memory written: [[ecraser-le-travail-dun-autre-agent]].

The other five NO-GO items are answered: the manifest now has a named signing authority
under Rule 36, the typed states have a failure state and eligibility begins at `verified`
rather than `sealed`, B1's transport is named because Rule 12's clause is scoped by its
own words to `tar.bz2` leaving a host and a chunk pull is outside it, the canon's
"universe" (the LXC) is reconciled with PodMesh's (a Podman container), and per-universe
keys are declared as B1's interim so no operator decision blocks it. The review also
corrected a claim I had made too strongly — dedup and per-universe erasability are *not*
mutually exclusive; a fourth encryption shape keeps both — and noted that `RULES.md:1214`
may already have settled X1 before it ever reaches Xavier.

**B1 still needs a third review** to move from NO-GO to GO: all six items are addressed,
but by the agent whose work was reviewed.

## Closed 2026-09-13 by Codex's decisions (`/tmp/podmesh-claude/DECISIONS-CODEX-2026-09-13.md`)

**M4 committed and pushed: `3504620`.** Codex closed GC-R1 to GC-R4 for the frozen hashes and
approved the lot, together with four clarifications now carried by GARBAGE-COLLECTION.md and marked
in it: what a dry run may write, class 2's eligibility (no authorization *live* rather than none
ever issued), which gate class 1 lifts, and two hold scopes (`evidence_hold` / `investigation_hold`,
an unknown hold blocking). The lot implements neither hold nor the widened class 2 and claims
neither. The closing review is `evidence/garbage-collection/independent-review-codex-2.md` (local;
`evidence/` is gitignored). Nine unit tests and clippy clean; no host suite was rerun, deliberately.
Also corrected in the same lot: LOCAL-API and MIGRATION-INTEGRATION said the migration operations
were "not packaged" when experimental5 carries them, and the checklist said 22 cleanup checks where
every run records 23. Not covered and not claimed: packaging, publication, installation, a third
host, classes 4 and 5, either hold, any timer, any production mandate.

**G2: the gate itself is amended, and that is a new lot.** Codex accepted that requiring
`incomplete_attempt_count == 0` makes a contractually required representation fail the gate, and
replaced it: G2 passes when `unaccounted_incomplete_attempts == 0`, scoped to attempt identities and
never to count deltas, with a deterministic eight-condition receiver-side join proving each
incomplete attempt is accounted for. Vocabulary: `terminal_attempts`,
`accounted_incomplete_attempts`, `unaccounted_incomplete_attempts`, `preexisting_incomplete_attempts`
(historical debt, retained and reported, never silently inside a success claim), `new_incomplete_attempts`.
The nine measured attempts are *eligible for re-evaluation*, not retroactively qualified. The work is
a **separate branch and lot** on `codex/web-console`, not to be mixed with M4 or Backup Server B1,
and needs a versioned evidence schema, a fail-closed comparator, positive and negative tests, and an
independent review. The unbounded pre-reply verification is its own separate lot; do not hide it with
timeouts and do not erase durable stores or retire activation markers without Xavier's explicit
destructive authorization.

**Backup Server designed and pushed: `7e4e294`.** From Rules 12 and 16 of the canon rather than from
Proxmox. See `docs/BACKUP-SERVER.md`; B1 is the first lot and must not be mixed with the G2 lot.

## Universe high availability (lots H1 to H8), 2026-09-14 — built, run on two lab hosts, not published

The operator asked for HA for the universes of his choosing, with the agent naming the host
and one or two standbys per universe depending on resources and on an allowance that is his
own judgement. The design is `docs/UNIVERSE-HIGH-AVAILABILITY.md`; the operations are in
`docs/LOCAL-API.md` under "activation leases and recovery points". Three honest levels:
planned handoff (level 1), warm standby from a recovery point (level 2), continuous disk
replication (level 3). Fencing is the whole safety problem; the agent's choice removes
election, never safety.

Built and checked, each rule verified by weakening it and watching its check go red at its
own case: activation policies and leases with a takeover margin (H1), self-fencing as an
operation the caller must drive (H2), replication intent plus host resource facts with the
allowance kept as provenance and never computed (H3), the lease following the migration
chain so level 1 cannot be raced (H4), `recovery_point_prepare` of a stopped universe into
an honestly **unsigned** point (H5), `recovery_point_restore` into a quarantined new
identity that refuses a tampered archive, a non-canonical manifest and a manifest claiming
a signature this build cannot verify (H6), and `recovery_point_promote` under the lease
(H7), and epoch-bound activation (H8): `experiments/manager-fencing` in the web tree (Codex,
2026-09-12) models exclusion as an epoch from one external gate rotated only by explicit
trusted action, with makers keeping a durable screen; PodMesh is now that maker — a policy
may name an `authority_id`, acquisition then needs a permit in the lab's exact form bound to
the universe, this host and this boot, the screen refuses superseded epochs and second grants
at one epoch, and `activation_supersede` voids this host's entitlement. Ten rules, each
removed and watched go red. Commits `c6e1e9c`, `6759318`, `05f6b01`, `ecb21c1` and the H8
commit after them.

What it does not do, stated in every answer: the lease proves this host's restraint, not
mutual exclusion — it is not replicated; no failure detector exists; no transport moves the
point (the two-host suite's controller can carry it); no signing crate exists in this build
and adding one is the operator's supply-chain decision; the manifest's origin is never
verified; PodMesh never verifies a permit's origin (a forged higher epoch can stop a universe,
never start a second one). On 2026-09-14 the whole level 2 sequence ran between two lab hosts
on a transient development service (`tests/check-recovery-point-two-hosts.py`; 28 checks on
the H7 binary, 37 with the epoch rotation on the H8 binary; reports in the gitignored
`evidence/ha/`), after a first run found that a standby with no activation history had no
activation tables — fixed, rebuilt, rerun. The transient units are stopped; their state
directories and the installed binaries under `/opt/podmesh-dev-ha/` were left in place.

## Collector class 5 for recovery points (lot M5), 2026-09-14 — built and checked, not published

Every capture leaves an archive in the outbox and nothing collected it: four Alpine runs left
33 MB per side, and a real universe is gigabytes per capture. The contract's class 5 is now
implemented **for recovery points only** (`src/retention.rs`, `src/collector.rs`, checked by
`tests/check-recovery-point-retention.py`): a declared retention (`keep_latest`,
`minimum_age_seconds`), **both hold scopes** interpreted in one function and applied to every
class, a retained manifest committed with the terminal state before any byte is removed, a
fresh re-hash immediately before the effect, a `max_bytes` bound, recovery that finishes an
interrupted removal rather than repeating a decision, and a prepare that replays from the
retained manifest. Six rules removed in turn and watched go red; the apply-time hold guard is
defence in depth for class 5 and the only rule for class 3 with a reclaim, which the single-host
check cannot reach — unit-tested and annotated. Checkpoint artifacts and inbox copies are not
collected; class 4 is still not implemented. Collector version 2, policy version 2.
Regression on two lab hosts the same day: Codex's `check-migration-collector.py` passed (79
checks) once it built the two survey shapes it had been finding among leftovers of earlier
suites on the shared journal (commit `0ffca5d`), and the HA two-host suite passed again (37).

## The agent's side as a tool (lot H9), 2026-09-14 — built, run on two lab hosts, not published

`tools/ha-standby.py`: `gate` (the fencing laboratory's `Authority`, imported from
`PODMESH_FENCING_LAB`, never copied), `activate`, `cycle`, `takeover`; one JSON report each, no
timer. `tests/check-ha-standby-tool.py` drove it on the lab hosts: refusals, two cycles with
pruning, takeover after a lapse with the active host reachable, and takeover with the active
host **unreachable** — 26 s waited on the standby's clock, the active host's own fence then
stopped its copy without escalation. The transient units are stopped again. The image a
restore imports on the standby is now removed by `delete` of its last user (H10), so a
standby's disk no longer grows by one image per cycle.

## Independent counter-review of lots H5–H10 and M5 (Claude Sonnet 5, read-only), 2026-09-14

Five findings, all verified and fixed the same day, each fix proven by removing it and
watching a check go red:

1. **The tool's takeover with the active host unreachable waited on its own CLI defaults,
   not on the lease and margin that had been activated** — the one finding with a second
   writer at the end of it. `activate` now records the policy in the ledger, `takeover` waits
   on that record and refuses without it; the lab check activates with 30/10 and takes over
   with no flags, and waits 41 s (a first version waited 26).
2. `activation_*` and the collection declarations did not keep the operations journal, so a
   retry of a release looked like a failure and a repeated ID with another request silently
   overwrote. All three modules now go through one shared `lifecycle::journaled` (same
   canonical-request comparison, verified replay as flat history, pending re-evaluated).
3. The recovery point replays looked up by operation ID without comparing the request —
   covered by the same helper; the per-table lookups remain as defence in depth.
4. `collect_point` could turn a committed collection into a refusal if the removal or the
   progress record failed after the commit; both are best-effort now, as for classes 1 and 2,
   and a removal that did not finish is a verification blocker the retry finishes.
5. `--keep 0` pruned nothing.

Not verified by the review, by its own statement: real partitions, real clock skew, and
concurrent submission. Regression after the fixes: seven local suites, clippy with warnings
denied, unit tests, and on the lab hosts the two-host suite (37) and the tool check
including the unreachable-host takeover.

## Hygiene before publication, as Codex directed on 2026-09-14 (`/tmp/podmesh-claude/NEXT-DIRECTION-AFTER-H10-2026-09-14.md`)

1. Six tracked `__pycache__/*.pyc` files removed from version control and ignored
   (`205e211`); no source or evidence altered.
2. Local checks rerun on `205e211` (debug binary `aaf8534afcd5…`), all exit 0:
   `PODMESH_STATE_DIR=<tmp> PODMESH_SOCKET=/tmp/pmr/api.sock PODMESH_JOURNAL=<tmp>/state.sqlite
   python3 -B tests/<suite>.py` for check-activation, -epoch, -fence, check-recovery-point,
   -restore, -promote, -retention.
3. Two-host development checks rerun on the lab hosts with the transient unit
   `podmesh-dev-ha` and the release binary `d312222d0f72…` (same source as `7181700`), all
   exit 0: `check-recovery-point-two-hosts` (37 checks), `check-ha-standby-tool` (8),
   `check-migration-collector` (79). Reports preserved under the gitignored
   `evidence/ha/after-hygiene-205e211/`. Units stopped afterwards; nothing installed.
4. Every commit stays local until Codex has reviewed the diff; nothing is published and
   no development service is installed as a service.

Status vocabulary for everything above: **implemented, locally tested, lab-tested** on two
transient hosts; nothing is deployed and nothing is production-qualified.

## M-U2, the networked replicated manager (Codex's direction of 2026-09-14) — steps 1 to 4 done the same night

The operator decided that networking is part of the normal universe contract; Codex's direction
sets an invariant (three replicas run concurrently, one governs) and an order. Done, one commit
per step: (1) `docs/UNIVERSE-NETWORK-CONTRACT.md`; (2) `src/network.rs` and the required
`network_profile` on `create` — isolated kept as it was, managed = a bridge per host on its `/24`
pool, one stable address per universe UUID, journaled declare/undeclare/route publish/withdraw
verified from `podman network inspect` and `ip route`, exactly one announcement per address;
(3) `tests/check-network-managed.py` on lab-a, 17 checks, host state restored; (4)
`tests/check-manager-replicas-managed.py`: three Alpine manager universes (musl resident,
`packaging/podmesh-manager/universe/replicated/` in the web tree generates the private replica
set) concurrently on three hosts, facts converged while all three ran, inspected from `podman cp`
copies by the attested inspector; (5) the governor role under the existing epoch gate with all
three running: `network_route_publish` with `exclusive_resource` is accepted only from the host
holding a live, unsuperseded lease on the resource (the logical manager UUID), `activation_fence`
withdraws exclusive routes of resources the host no longer holds, the tool's `rotate` moves the
role without promoting or starting anything; `tests/check-manager-governor-managed.py` proved
exactly one announcement of the service address at every observed moment, the old governor's
withdrawal before the new publication, the stale permit and the old governor refused, all three
replicas running and still converged across the takeover; (6) in the same suite, a duplicate
address refused, and a replica stopped and restarted converging to a fourth fact on all three.
(7) `recovery_point_promote` carries the managed profile (required `network_profile`, optional
`network_address`), the restore reports the source's network, a released allocation no longer
blocks re-allocating the same universe; `tests/check-manager-recovery-managed.py`: a replica
stopped, captured, deleted, the set moving on without it, then restored, promoted at its own
address and started, catching up the fact it missed and converging on all three while the
governor's announcement never moved. All seven steps of Codex's order are done. Then, on the
operator's "continue at all costs" (2026-09-15, night), the agent's door to the manager:
`manager_status` and `manager_observe` (`src/manager.rs`, `LOCAL-API.md`), typed operations
relayed by a copy of the daemon entered into the universe's PID namespace — the resident refuses
a peer whose PID it cannot see, measured, not assumed; `tests/check-manager-control.py` on lab-a:
an observation named by the agent in the store beside the boot fact, replay served from the
journal, a second one appended, six refusals each for its own reason, the resident's refusal of
a scope not owned returned as such (mutation: that check removed went red there). Then the
replica answering at the service address: the exclusive route gives the governor's replica the
address as an alias inside its network namespace, withdrawn with the route; the replicas listen
on every address; `tests/check-manager-service-address.py` on the three hosts: a TCP connection
to the service address from the other hosts is accepted by the governor's replica only, before
and after the role moves with all three running, and fails at once when nothing is announced
(two mutations red at their own checks). Then a real partition
(`tests/check-manager-partition.py`): the governor's host cut from the two others by an nftables
table with a dead man's switch, the agent still reaching every host — the connected replicas
diverged from the cut one, the service address was unreachable across the cut, the role moved
with the cut host fenced on request, and on reconnection the cut replica converged as a simple
replica. Then the source NAT removed inside the prefix: the declaration creates an nftables
table (`notrack` for prefix-to-prefix traffic) and the undeclaration removes it, both verified;
`tests/check-network-no-nat.py` on two hosts saw each universe with its own address at the
other's. Then the self-fence on a timer under a mandate, packaged disabled
(`packaging/podmesh-fence`, `podmesh-fence.service`, `podmesh-fence.timer`; the postinst never
enables it): `tests/check-fence-timer.py` on lab-a — no mandate, nothing fenced; lease live,
nothing withdrawn; lease lapsed on the host's clock with nobody calling anything, the role's
route and the carried address gone within one interval, the universe still running. What that
closes, once the operator enables it: a partition that also cuts the agent from the governor's
host. Not shown: a host loss, an authenticated exchange at the service address, a signed
manifest, the packaged units themselves under a real installation.
Status: implemented, lab-tested on three hosts; not deployed, not
production-qualified; nothing published.

## Codex's review of M-U2 (2026-09-15): GO for the lab result, NO-GO for packaging until B1–B3

`/tmp/podmesh-claude/CODEX-REVIEW-M-U2-AND-DIRECTION-2026-09-15.md`: three blockers (B1 kernel
effects surviving without a durable record; B2 replica keys baked into image layers; B3
`notrack` wider than "no source NAT"), four important findings (I1 the timer depends on the
daemon — name it lease-expiry self-withdrawal; I2 a journalled fence every five seconds grows
without bound; I3 the control relay's PID reuse window; I4 stale HA doc closure text), the
operator's eight decisions applied, and a ten-step order. **B1 done** the same day: the
effects ledger (every kernel mutation recorded and committed before it is made), compensation
in every error path, reconciliation at startup, before every mutation and at every fence, lab
fault injection, and `tests/check-network-crash-safety.py` (eight fault cases, each finished or
undone by the restart). **B2 done** the same day: one generic Alpine image with no
configuration and no key; PodMesh secrets (`secret_declare` from a root-only inbox file into
Podman's store, `create` with `secrets` mounted root-only at a target and labelled by name,
`secret_remove` refused while carried, `secret_status` without content; restore reports the
source's secrets by name, promote takes them again); `tests/check-secrets-image-free.py` (the
image save and the container export scanned for every pair key and identity) and every manager
suite rerun on the generic image with secrets. **B3 done** the same day: the source-NAT
exemption's default is a null source NAT of the local pool's traffic to the prefix (connection
tracking kept), `notrack` selectable with its consequence stated, `none` explicit;
`tests/check-network-nat-matrix.py` on two hosts measured both backends — TCP and UDP both
directions, a stateful firewall passing under `null-snat` and blocking under `notrack`,
Podman's NAT kept outside the prefix and for host-address traffic, the table intact after a
fence's reconciliation, removed by the undeclaration. The rest follows in Codex's order.

## The publishing connector follows the governor (2026-09-15, the operator's decision)

`docs/MANAGER-PUBLISHER-CONTRACT.md` and `src/publisher.rs`: one Cloudflare tunnel, one public
hostname, exactly one `cloudflared`, co-located with the governor replica, started only under
the live unsuperseded lease with the service address effective, the previous publisher accounted
for, the governor mark written inside the carrier and the origin answering ready at the epoch;
stopped by the fence before the address goes. Measured on lab-b and lab-c with a real laboratory
tunnel: an external request through the public hostname answered with the governor's replica and
epoch, then, after the rotation and the old governor's fence, with the new governor's. Each gate
mutated in turn went red at its own check. The hard test passed: the governor's host cut from
its peer and the agent with its Internet kept, its own timer withdrew the connector 5.8 s after
the lease lapse and 30.5 s before the standby published; the hostname answered 530 in between,
then the standby's replica at the new epoch. I1–I4 of Codex's review are done the same day
(lease-expiry self-withdrawal named, the fence preview, the relay bound to the container's
identity, the HA document reconciled). The suites of the day ran on lab-b and lab-c. Their first
agent-cut run had left lab-a isolated by an nftables table whose dead man's switch had not been
armed. Codex restored access on 2026-09-15 through the VM guest agent, then used the PodMesh API
to stop and delete only that campaign's universe, remove its secret and network declaration, and
stopped its transient fence timer and development daemon. The pre-existing observation service
and older proof containers were left untouched. The corrected suite arms and verifies its switch
before applying the partition.

## The manager laboratory hostname (2026-09-15, Codex verification)

The operator authorized `manager.szde.fr` as the manager laboratory hostname. Codex verified the
existing Cloudflare API token without exposing it, created a proxied CNAME to the existing locally
managed `podmesh-lab` tunnel, and ran `tests/check-manager-publisher.py` on isolated development
daemons on lab-b and lab-c. Exit 0: the public `/ready` response named lab-b and epoch 1, the fence
withdrew that publisher, then the same hostname named lab-c and epoch 2. Cleanup restored both
hosts' network state and left both connector units inactive; the retained hostname then returned
Cloudflare 530 because no connector was running. The raw result is
`evidence/manager-publisher/2026-09-15/manager-szde-fr-takeover.json`. This proves the current
laboratory transition only; the unverified `previous` assertion and crash-finalization findings in
`/tmp/podmesh-claude/CODEX-REVIEW-B1-B3-CLOUDFLARE-2026-09-15.md` still block signatures and
packaging.

## Codex's second review applied (2026-09-15): P0, three P1, P2

`/tmp/podmesh-claude/CODEX-REVIEW-B1-B3-CLOUDFLARE-2026-09-15.md` accepted the locally managed
tunnel and the `previous` account as provenance only, and blocked signatures and packaging on
five findings. All five are done and measured on the reference build `cdc980d3…` then
`campaign 6` (below). **P0** — the unverified `previous` gate is replaced by the authority's
typed takeover proof: the tool's `rotate` issues it (resource, both epochs, both holders,
method `first`/`same_holder`/`lease_barrier` with `eligible_after` = the previous lease plus the
margin from the rotation, issue and expiry), `attest-fence` upgrades it to `fence_receipt` from
the previous holder's fence answer (the resource must be among the fence's `unentitled`, no
withdrawal failed), and `publisher_start` checks every binding; a proof that is absent, waited
before its barrier, for another resource, stale, for another holder or expired is refused, each
at its own reason (`check-manager-publisher.py`, `check-manager-publisher-agent-cut.py`).
**P1 transitions** — `publisher_transitions` records `starting` before any effect and
`effective` last; seven lab faults (after the mark, after the connector, before the final
state, each as a failure and as a crash, and a crash during the compensation itself — two fault
points at once) leave nothing: a failure compensates and reports it, a crash is withdrawn by
the restart's reconciliation, and an `effective` publisher whose lease is superseded is
withdrawn at the next restart (`check-manager-publisher-crash.py`, eight cases). **P1
secrets** — a state machine `declaring`/`effective`/`removing` with the store's digest verified
against the intent, immutable names (same content idempotent, other content refused), upsert
over a removed row, compensation of an uncommitted store, reconciliation at startup
(`check-secrets-crash.py`). **P1 registration** — `publisher_start` waits, bounded, for the
connector's registration identity in `cloudflared`'s journal; a unit that exits or never
registers is withdrawn and reported. **P2** — the NAT exemption is verified as the exact rule
set for the backend, prefix and pool (`nft list table` compared to the expected rules, chain
lines excluded); three mutations (wrong pool, wrong prefix, an extra rule) each went red at the
declaration. Also from the day's runs: the fence's own pre-mutation reconciliation withdraws a
superseded publisher before the fence's step reaches it (the supersession having been delivered
first); that withdrawal is now reported in the fence's `publishers_withdrawn`, labelled with the
pass that made it, so the contract's field holds whichever pass did the work. Two suite
corrections were the day's real findings about the suites, not the daemon: a crashed unit has no
socket, so a state read after an injected crash must come from the journal itself; and the
proof's barrier (25 s) outlasts a 20-second lease, so the new holder acquires again with the
rotation's permit after the barrier — the design's own answer to a lapsed lease of one's own,
idempotent for the holder.

## The takeover document signed (2026-09-15, Codex's step 4)

`src/signing.rs` verifies an Ed25519 signature (`ed25519-dalek` 2.2, verification only; the
first dependency beyond `serde_json` and `rusqlite` — a supply-chain choice for the operator to
confirm) over a document's canonical form: the document without `signature`, every object's
keys sorted, compact JSON, no floating-point number; the tool's `sign` produces exactly that form
(a document signed by the tool's Python and verified by the Rust unit test settles the
canonicalisation: accents, escapes, nested objects, null and booleans). A policy names the
authority's public key (`activation_require` `authority_key`, refused unless a valid point and
unless an authority is named); the tool's gate generates its signing key at first use
(`PODMESH_HA_KEYS`, one 0600 file per authority, never printed) and names its public half in
every policy it declares; `rotate` and `attest-fence` sign every proof. Under a keyed policy
`publisher_start` accepts only the signed kind, verifies the signature **before** reading a
field, then checks the binding; under a policy without a key only the unsigned laboratory kind,
labelled `signed: false`. `tests/check-takeover-proof-signature.py` on lab-a: a malformed key, a
non-point key (y = 2) and a key without an authority refused at declaration with the policy
unchanged; an unsigned proof, an altered one (a field no binding reads), one signed by an
unknown key, a zero signature and a missing signature refused before binding; documents signed
by the real key for another resource, expired, for another holder and for the next epoch refused
at their binding; the genuine document accepted with the signature recorded as verified. The
permit itself stays unsigned provenance, as `LOCAL-API.md` and the HA document say. Recovery
point manifests stay unsigned.

**Campaign 6 (2026-09-15, build `54ef30ed…`, reproduced byte-identical before and after the
mutation, installed as the isolated transient service on lab-a, lab-b and lab-c):** secrets-crash,
publisher-crash (eight cases), readiness, the signature suite (thirteen checks), agent-cut (the
real cut with the signed barrier proof and the re-acquire) all PASS; the mutation removing the
signature check went red at the altered-document case (the mutated daemon accepted an altered
document; the suite's cleanup left no connector); the twelve-suite regression (no-nat,
nat-matrix, network-managed, crash-safety, secrets-image-free, governor, service-address,
recovery, partition, control, control-race, fence-timer) all PASS on the same build. The
three-host publisher suite, whose binding cases alter proof fields and now re-sign them with the
gate's key, ran last on that build: PASS — lab-a governor, lab-b standby, lab-c following; every binding refused at its own reason with a re-signed document, the fence's `publishers_withdrawn` carrying the reconciliation's withdrawal, the public hostname answering the new governor at the new epoch, the permit gate alone at the end.

**Cursor reproduction (2026-09-15 night, same isolated `podmesh-dev-ha.service`, same replica kit, same build `54ef30ed…`):** kit suites from secrets through publisher-crash, partition, signature, publisher, publisher-agent-cut and partition-agent-cut all PASS live. Evidence: `podmesh-lab/cursor/campaign-cursor-2026-09-15T1905Z/` and `…T2138Z/`; handoff `podmesh-lab/cursor/CURSOR-HANDOFF-2026-09-15T2210Z.md`. Partition-agent-cut failed three times because the one-line nftables chain body does not parse on the lab; the harness now writes a multiline ruleset, checks it with `nft -c`, and applies it from `systemd-run` (uncommitted). This is a second live run of Campaign 6's lab claims, not a packaging or production-HA claim. Local `agy` produced T1–T4 reading notes only.

**Branch divergence found while reconciling the inventory (`docs/README.md`):** the package
installed on the three hosts, `0.1.0~experimental6`, was built by Codex on 2026-09-12 from
`codex/collector-completion` (`a804f0b` + `4d7310b` durable collector recovery and pidfd-bound
reclaim signals + `ea2104c`), and `main` does not contain those two commits; `main`'s collector
(M4 and its amendments) is another implementation. Which collector ships is Codex's decision
before step 8.

## Next actions, in order

1. Done: independent read-only counter-review of the source-side milestone (Claude
   Sonnet 5), findings verified and applied, suites rerun, milestone committed.
2. Done: lot M2, the destination side, implemented, independently counter-reviewed and
   committed. Deviations are recorded at the end of MIGRATION-PROTOCOL.md.
3. Done 2026-09-12, commit `4991929`: lot M3. Release, abandonment and local restore of a
   reservation that never left its host; a bound that freezes a runaway restore; an explicit
   `reclaim_processes` that ends only what it can prove; a read-only `watch` object. Eleven
   suites on one binary (36, 23, 66, 26, 35, 8, 6, 13, 40, 25, 9), independently
   counter-reviewed (Sonnet 5, no blocking defect), its two findings fixed and the suites
   rerun. The operator answered both open questions on 2026-09-12 with one word, garbage
   collector: neither dead end gets its own operation; both become cases a collector sweeps on
   proof, never on age. The operator's full contract is GARBAGE-COLLECTION.md.
4. Done 2026-09-12, not committed: lot M4, the collector's plan, terminal reservation
   classes 1 and 2 with their tombstones, and class 3 delegated to the existing abort.
   Codex's four findings fixed and the suites rerun; its closing review is the gate, and
   publication is withheld until then. See the M4 section above. Artifact retention with
   evidence holds (class 5) is done for recovery points on 2026-09-14 (M5 section above);
   a failed local restore (class 4) and checkpoint artifacts remain.
   No autonomous timer in either.
   Two documentation corrections now fall to me, once the review closes and before the
   commit: LOCAL-API.md still says the migration operations are "not packaged" at its two
   migration headings, which experimental5 made false; and DELIVERY-CHECKLIST.md's M3
   sentence says the cleanup suite has 22 checks where every later run records 23.
   Superseded record of what this lot set out to do: release, abandonment and local restore; failed-restore cleanup as a
   verified reclaim bounded by cgroup membership behind an explicit default-false request
   field, combined with prevention measured on the hard failure shape; the two coverage gaps
   the review named (a repeated `migration_complete_transfer`, and a re-authorization after
   `not_restored`). Resource incidents keep their `df` and journal captures as evidence.
   The operator decided on 2026-09-12 that the root tandem, the human with the vibecoder
   agent under his direct control, is the authority that may request a reclaim. The request
   records that provenance; it does not replace the proof. PodMesh still refuses to signal
   anything it cannot prove belongs to the failed attempt, whatever the requester claims.
4. Package and qualify each accepted increment through signed APT; publish sources,
   tests and honest limitations together after review. **Codex's order of 2026-09-15 governs
   the way there** (`CODEX-REVIEW-B1-B3-CLOUDFLARE-2026-09-15.md`): steps 1–4 (P0, P1 ×3, P2,
   the signed takeover document) and 6 (the inventory in `docs/README.md`) are done and
   measured (sections above); step 5, movable replica recovery and an actual VM loss, needs the
   operator to name the VM that may be destroyed; step 7 is Codex's final diff review
   (`dff3e65..HEAD`); step 8, the Debian 13 package and the Alpine manager image, only after it,
   and after Codex decides which collector ships (the branch divergence above).
5. Control-services universe, per CONTROL-SERVICES-UNIVERSE.md (operator design
   direction and complete brief, 2026-09-12). Its own order: registry schema and an
   immutable event format, one local instance with export and import, restart and
   recovery from its own backup, replicas on two then three hosts, DNS publication
   from verified facts, then partition and reconnection. Its section 14 forbids the
   implementing agent from inventing the open values; two of them block the first
   step and are the operator's: accepting SQLite plus an append-only event history as
   the first store, and the exact event/conflict format. One row I would add to that
   table: what a partitioned replica does with DNS — keep serving, serve marked stale,
   or stop publishing — since two partitions that both publish can hand different
   addresses for one name to different clients, which freshness rules alone do not
   prevent. The brief's section 9 answers the earlier registry/network overlap: they
   are specified in the same document and should be built in the same lot.
6. Continue the central checklist: networking/volumes, partitions and HA, fractal
   demonstration, sequential storage tests, Backup Server and product documentation.
7. HA, in order: done — level 2 across two lab hosts, epoch-bound activation, the
   collector's class 5 for archives, and the agent's side as a tool driving the
   laboratory's gate. What remains is the operator's (UNIVERSE-HIGH-AVAILABILITY.md, "What
   is the operator's to decide"): which gate for production, the signing crate, the capture
   interval and retention per universe, out-of-band fencing and who may cut power, which
   universes, and whether the tool may ever be a timer. Then Codex's review, packaging, and
   level 3 after B0 qualifies a storage backend.
8. Manager pre-reply verification (Codex's candidate): the web tree's
   `docs/MANAGER-PRE-REPLY-VERIFICATION.md` gained an addendum on 2026-09-14 — a durable
   digest cannot detect an edit to a row the verifier does not read, so an O(new) reply
   path is honest only if full verification leaves the reply path and the contract states
   a detection interval. Two shapes and a recommendation are recorded; the choice is the
   candidate's contract and Codex's.

## Cost and delegation policy

The operator reports almost half the weekly GPT allowance used; this is not a
live usage measurement. Preserve the remainder. Prefer Claude Code installed with
Cursor, authenticated through the existing Claude Max subscription, for bounded
implementation and tests. Do not substitute usage-billed API keys without explicit
authorization. Codex retains difficult architecture, coordination and verification.
Choose and announce model and effort per task; do not assume inherited settings.
Use targeted context and evidence, avoid frequent unchanged polling and unnecessary
repeated tests. Claude Opus 5 medium implemented the recent milestones; Sonnet 5
medium handled helper fixes and independent review. No new delegation is launched
merely by this handoff. Recommended next: protocol design with the operator, Opus 5
high; independent diff review, Sonnet 5 high, read-only; destination preflight and
verified transfer, Opus 5 medium; restore and exclusion, Opus 5 high; packaging and
three-host qualification, Sonnet 5 medium.

## Durable references

- INTENT.md: product intent.
- DELIVERY-CHECKLIST.md: central requirements and execution tracker.
- LOCAL-API.md and MIGRATION-INTEGRATION.md: API and migration boundaries.
- ACCEPTANCE-TEST-PLAN.md and REVIEW-EXPERIMENTAL4.md: acceptance and qualification.
- NETWORK-AND-PLACEMENT.md, BACKUP-SERVER.md, LVM-LAB-PLAN.md: future work details.
- Local ignored `evidence/handoffs/2026-09-11/`: full implementation/review reports
  and the checkpoint mission, copied out of temporary storage.
- Local ignored `evidence/experimental4/` and `evidence/migration-source/`: raw
  results. Internal addresses and runtime state paths remain there, not in public
  configuration examples.
