# The three-host manager takeover experiment: design, and where it stops today

Status: **designed; the universe half is exercised on three lab hosts; the manager half waits
for a decision on the candidate.** Codex's third engineering item of 2026-09-14: *design and
test a three-host manager takeover experiment; it needs an external authority/fencing decision,
durable epoch screening, an old-active exclusion proof, a bootstrap path independent of manager
DNS, and an explicit reconciliation rule after a partition; do not replace these with a numeric
priority.*

## The five requirements, and what exists for each

| requirement | exists | where | measured |
| --- | --- | --- | --- |
| external authority / fencing decision | the fencing laboratory's `Authority` gate (one SQLite compare-and-swap, explicit rotation, 22 tests); the self-fence with a takeover margin | `experiments/manager-fencing`; PodMesh `activation_fence` | two- and three-host checks in the main tree |
| durable epoch screening | PodMesh is the maker: `activation_epochs` screen, permit bound to universe, host and boot, one grant per epoch | main tree `src/activation.rs` (H8) | ten rules mutated; lab runs |
| old-active exclusion proof | supersession voids the lease in the gate, the renewal and the fence; a stale grant, a fresh permit at the old epoch and a second grant at the new epoch are all refused | `activation_supersede`, `check-ha-three-hosts.py` | HA-10 shape on three hosts, without a real partition |
| bootstrap independent of manager DNS | the G2 configuration is static IPv4 peers under `IPAddressAllow`; nothing resolves a name | `activation/README.md`, G2 drop-in | six campaigns |
| reconciliation rule after a partition | `Reconciliation::after_full_exchange`: only after every declared replica holds the identical history; the coordinator is the lowest replica ID **of that agreed set**, and it decides nothing about which data is true; a permit dies with the history it names | `manager-ha/src/lib.rs` | model only (G0); no partition campaign |

Nothing above is a numeric priority. The lowest ID selects a *coordinator* after the histories
are already identical; it never selects a winner between diverging histories, and it grants
nothing by itself.

## Why the manager half cannot be tested today

A takeover of the *manager* means an exclusive effect of the manager — publishing the active
route, answering as the authority — moves from one replica to another under the gate. The
model has it: a fact with `exclusive_resource` and `active_claim`, and
`authorize_exclusive_service` issuing a permit to the reconciled coordinator. **The packaged
resident exposes three control operations — `status`, `shutdown`, `append_observation` — and
no permit path**, and campaign 6 carries zero exclusive facts (`MANAGER-REPLICATION-DATA-PATH.md`).
There is no effect to move, and no gate call the resident makes.

Two ways to get one, both Codex's since either changes the candidate:

1. **Expose the permit path in a new candidate**: a `check_service` control operation that
   asks the reconciled view for a permit and an effect (a route, a marker file, a counter as in
   the laboratory) that is only performed under it, screened by epoch. Then HA-04/HA-10 run
   against the manager itself.
2. **Run each manager replica as a PodMesh universe** — the intended architecture says the
   manager is a ShaperOS universe replicated to each chosen host — and let PodMesh's
   activation screen be G3's enforcement point for it, with no new manager code: the replica
   process is the universe, its exclusive role is the activation lease under the epoch gate,
   its takeover is the tool's. The replication data path stays what campaign 6 qualified.

The recommendation is the second: it uses only what is built and measured, keeps the
candidate frozen, and turns the manager's HA into an instance of the universe HA rather than a
second mechanism. Its cost is an operational one — the manager's configuration, socket and
state directory move into a universe — and that is exactly the kind of decision this document
leaves where it belongs.

## What is exercised today, on three hosts

`tests/check-ha-three-hosts.py` in the main tree drives `tools/ha-standby.py` across lab-a,
lab-b and lab-c on a universe: one capture cycle restored into quarantine on both standbys with
the marker present; the active host lapses; one standby takes over under epoch 2 and the other
is informed and refuses a stale epoch-1 permit bound to it; the old active rejoins **as a
standby** — superseded, refused under its old grant, under a fresh permit at the old epoch and
under a second grant at the new epoch, its copy fenced, and a cycle from the new active
restores into quarantine on it without any journal reset; the gate stands at epoch 2. That is
HA-10's shape without a real partition: the active host "fails" by not renewing, and no
network was cut.

## Measured after Codex's decision: candidate M-U1, the manager as a universe

Codex chose the second way the same evening. `packaging/podmesh-manager/universe/` is the
universe definition, and `tests/check-manager-universe-ha.py` (main tree) ran it on three lab
hosts: the packaged resident runs inside a PodMesh universe with no network; a capture cycle
stops it gracefully — the entrypoint turns the stop signal into the typed shutdown — and
restores the point on both standbys; the frozen candidate's own inspection of the exported
stores shows two boot facts on the active host, three in the promoted universe on the standby
before its first start, four after it started there, chained, integrity ok; the other standby
refuses a stale permit and the old active is refused. **The manager's durable state follows the
universe through capture, restore, promotion and restart on another host**, under the same
mechanism as any universe and with no manager-specific election.

Codex reviewed that run the same night — GO for the narrow claim, NO-GO for packaging until
four corrections — and the corrections were made and rerun on the three hosts: the binary
inside the universe is attested byte-equal to the inspector before any inspection; a boot fact
that is not observed makes the start fail and a typed shutdown that is not acknowledged makes
the stop an honest failure the capture cycle refuses after; the configuration is labelled a
single-replica portability fixture; and Alpine was tried first, failing on the candidate's
glibc ≥ 2.34 dependency (`ALPINE-PROOF.md`), which is the recorded reason for the Debian image.

The operator's image policy of the same day (Alpine root by default, Debian only as a local,
documented exception) was then applied: the resident was built for musl from the frozen source
commit in an Alpine Rust container, an Alpine root image (60 MB) passed the identical three-host
proof with that binary attested as both resident and inspector, and the Debian image became the
compatibility branch for the frozen glibc candidate. Freezing the musl build as a candidate is
Codex's.

**M-U2, step 4 (the same night):** on the operator's decision that networking is part of the
universe contract, PodMesh gained a managed profile (main tree, `docs/UNIVERSE-NETWORK-CONTRACT.md`),
and three Alpine manager universes ran **concurrently** on it across the three hosts as three
replicas of one logical manager, with explicit authenticated endpoints: their facts converged
while all three kept running. The replicated manager exists inside universes.

**M-U2, steps 5 and 6 (the same night):** the governor role, under the same epoch gate as any
universe, with all three replicas running. The activation resource is the logical manager's
UUID; the exclusive effect is the announcement of its service address, a `/32` route that only
the host holding a live, unsuperseded lease on that resource may publish, and that the host's
self-fence withdraws once the lease is gone. Measured on the three hosts
(`tests/check-manager-governor-managed.py` in the main tree): one announcement at every
observed moment; on the rotation from epoch 1 to epoch 2 the old governor's fence withdrew the
route before the new governor published it; the old governor was refused when it tried to
publish again, a forged stale permit was refused on the third host, which was also refused
without the role; the three replicas never stopped, and their facts were still converged after
the takeover; a duplicate address was refused; a replica stopped and restarted rejoined with a
fourth fact on all three. What this proves is the exclusivity of the announcement; the replica
does not yet serve at that address (an address alias inside the container is a later step), and
the loss exercised is a stop, not a cut.

Three findings the candidate taught, each handled in the universe definition and recorded in
its README: the resident handles no signal, so PID 1 must translate the stop; the overlay's
copy-up changes a restored store's inode between the resident's preflight and its open, which
it refuses as a swapped store; and the control socket is unreachable from the host, so the
only writer inside is the entrypoint. What still needs a universe contract decision: a network
(without one, replicas cannot replicate inside universes and the campaign-6 data path is not
exercised there) and a way for an agent to reach the control socket.

## What it does not show

A real partition, a real host loss (HA-04 requires power loss, not network loss), the manager
serving at its service address (only the announcement is exclusive so far), DNS, or a long run. The latency curve of the pre-reply verification is
unchanged and is the other thing that decides whether this manager can run for a day.
