# Publisher follow (laboratory)

Status: **lab mandate, 2026-09-16**. Not a production timer and not a second election.
Owner: Xavier de Poorter.

The publishing connector follows the active manager (`MANAGER-PUBLISHER-CONTRACT.md`). The
daemon still **does not** start a connector by itself: `publisher_start` needs the authority's
takeover proof. What was missing for a standing laboratory hostname was a **host-side tick**,
under a written mandate, that applies that proof when **this host is eligible**, and stops
the connector when it is not.

## Decision (D1)

On `podmesh-dev-ha` only, the operator authorizes a follow tick analogous to `podmesh-fence`:

- It refuses to run without the mandate file.
- It never invents a proof. The gate's `rotate` / `attest-fence` document is installed on the
  host as a root-only file (`proof=` in the mandate).
- If `publisher_status` says eligible and the unit is already active, the tick checks what
  eligibility does not (V3-1, below) and is otherwise empty.
- If eligible and the unit is not active, it calls `publisher_start`: with the installed proof when
  its `new_epoch` matches the live lease epoch, and without one otherwise, the node then resuming
  under the proof it already verified for that epoch, or refusing and saying why (V3-1, below).
- If not eligible and a connector is recorded or active, it calls `publisher_stop`.
- If `renew=1` and this host is eligible, the tick calls `activation_renew` so a standing
  laboratory hostname does not die with the first lease. A host that is not the holder
  never renews. **The renewal is bounded, and a mandate that renews without bounds is
  refused.** `not_after` is the wall-clock second the mandate dies; after it the tick grants
  nothing and renews nothing, so what is published runs out its lease and the tick's own stop
  branch withdraws it. `renew_below` is how near the lease's end a tick renews, which also
  keeps the journal from growing by a row every ten seconds. Counter-review of 2026-09-16:
  measured unbounded, the holder's lease moved forward faster than it burned (+21 s over 14 s
  of clock) and could never lapse; bounded, it burns down and renews about three times an hour.
- Enabling the timer is the operator's; cleanup stops the timer and removes the mandate.

**What an armed renewal costs, and why it is bounded.** Lease expiry is the only thing that
withdraws an active manager nobody can reach: eligibility is read from this host's own journal,
so a host cut from its peers and from the agent stays eligible for as long as its lease lives.
With an immortal lease it would keep publishing while a standby takes the role at the barrier,
and Cloudflare accepts both connectors on one tunnel -- the case the invariant of
`MANAGER-PUBLISHER-CONTRACT.md` forbids. The bound restores the lapse. It does not make the
demonstration free: while the mandate stands, an active manager lost for real is withdrawn only
when its lease runs out, and a standby may publish only at the barrier (the lease plus the
margin recorded at the rotation), so a long lease buys a stable hostname with a long outage.
Choose the lease for what is being shown, and keep `podmesh-fence` in mind for the case where
the tick itself cannot run.

This does **not** rotate the epoch or publish the exclusive route for the first time. Those
remain the agent's (`tools/ha-standby.py`). After a rotation the agent must deliver the new proof
to the hosts (`python3 -B tools/arm-publisher-follow.py --refresh`). A lapsed lease still
withdraws through the tick's stop branch, fence or reconciliation -- provided the mandate is not
renewing it out of reach of expiry, which is what the bound above prevents.

## The entry point follows its holder (V3-1, 2026-09-18)

Measured on 2026-09-17: the takeover proof lives one hour while the lease it started is renewed
without any proof, so a connector that died after that hour came back only through a new rotate;
and a stop and a start of the active manager's replica destroyed the service address (the alias
lives in the carrier's namespace, the /32 went with the bridge's interface), which nothing put back,
so the tick stopped the connector, stopped renewing, and the page stayed down until `--refresh`.
While the holder does not change, the tick and the node now bring the page back by themselves:

- **The node resumes at the same epoch.** `publisher_start` records every proof it verifies with the
  lease incarnation (epoch, generation, `acquired_at`), the boot and the policy's authority and key.
  A start without a proof, or with one refused, resumes under that record while the lease is live,
  held here and unsuperseded, at the same epoch, generation and acquisition, during the same boot,
  under the same authority and key -- the conditions under which a connector that never stopped
  continues -- and is refused naming the first condition that fails otherwise
  (`MANAGER-PUBLISHER-CONTRACT.md`).
- **The node resumes the service address.** When the service address is the only gate missing
  (`publisher_status`'s `gates`), the tick calls `network_route_resume` once: the resource's recorded
  exclusive route and alias made again in place with the recorded ip, via and resource -- a failure
  leaves the row recorded for the next tick's resume -- only while the lease is live, held here, unsuperseded and was acquired or renewed during this boot,
  a universe runs at `via`, and the kernel holds no other route for the address. A refusal (the
  replica not running yet, say) is reported and the tick goes on: a connector of a host that is not
  eligible is stopped as before.
- **The tick checks a running connector.** Eligible and active, it compares, from
  `publisher_status` alone, the mark's epoch with the lease's, the origin's readiness at that epoch
  (200, ready, this logical manager, the carrier's replica) and the connector's registration in its
  **current** run (the registration of an earlier run of the same unit no longer counts). On a
  positive mismatch -- no mark, a mark at another epoch, an origin that answered otherwise -- it stops
  the connector at once; the next tick starts it again under the resume rule. A reading that could not
  be made (a `podman exec` that failed, an origin not reached, a replica identity not known, a journal
  not read, or a current run whose journal shows no registration) is not a mismatch: the connector is
  stopped only when it persists three ticks in a row, counted in `/run/podmesh-publisher-follow/
  tick-state.json` (`PODMESH_PUBLISHER_FOLLOW_STATE`), and a good reading resets the count. A node
  older than those fields is not second-guessed.
- **A failed start waits.** A start that cannot register holds the one-request-at-a-time daemon for
  up to 60 seconds; after a failed start the next waits 20 s, doubling up to 300 s, in the same state
  file; a success or a new epoch clears it. An installed proof file that cannot be read or is not a
  document is started without, and said once per content; it never stops the tick.
- **The replica claims nothing at its start.** The manager universe's entrypoint removes the mark,
  at both paths and at the path `PODMESH_ACTIVE_MANAGER_MARK` overrides them with, before the origin starts; the origin answers 503 until PodMesh writes the mark again
  at `publisher_start`.
- **The daemon withdraws at its start.** A connector whose lease lapsed while the daemon was down is
  withdrawn, connector and mark, in one journaled operation before anything is served.

Renewal is unchanged: only while eligible, inside `renew_below`, before `not_after`. A replica that
stays down therefore stops the renewal, and the lease lapses on its own clock: the page comes back by
itself only if the replica returns before that -- with `renew_below` 900 and a 3600 s lease, within
about 890 s to 3600 s of its stop, depending on where the lease stood. The mandate's
`not_after` stays the human bound: after it the tick renews nothing, resumes nothing -- neither a
route nor a connector -- and starts nothing; it still stops what is not eligible or does not match.

What this lot does not do: it never changes the holder, never acquires, never mints or extends a
proof, and never renews past `not_after`. **After a reboot of the holder's host nothing resumes**:
the tick, the mandate and the proof were under `/run`, the proof was verified during another boot,
and the ledger's /32 is withdrawn at boot and never re-applied; the entitlement is decided again, by
the gate today (`--refresh`).

**A rotation to another holder, and the mandate this tick renews under.** Review of 2026-09-18: with
the resume, a holder whose replica returns while its lease lives becomes eligible again and renews;
if a rotation elsewhere was never delivered to it, its lease runs past the barrier counted from the
rotation, and two connectors publish one tunnel. `tools/arm-publisher-follow.py` now records every
mandate it issues (host, `not_after`, no secret) in the resource's ledger before installing it, and
`tools/ha-standby.py rotate` to another holder delivers the supersession to the previous holder
(`--previous-host`) before any proof is made -- a superseded host renews nothing and resumes nothing
-- or, when it cannot, makes the barrier no earlier than that mandate's `not_after` plus the lease
plus the margin, and refuses (`follow_mandate_unknown`) before the gate moves when the record cannot
be read. A mandate the ledger does not record (armed before 2026-09-18, or from another workstation)
is stated with `--follow-mandate-not-after`. The closure that needs no workstation, a renewal the old
holder cannot grant itself, is a later lot. Proven without the laboratory: the unit tests of the resume rules and of
the route resume's refusals, and `tests/check-publisher-follow-script.py` against a stubbed CLI.
Only the laboratory proves the page coming back, the route and alias re-made in a restarted
carrier, and the registration read from a unit's current run.

## What it is not

- Not automatic production HA and not a claim that Cloudflare is always on.
- Not a replacement for `check-manager-publisher.py`.
- Not packaged in experimental7; the script lives in `packaging/podmesh-publisher-follow`.

## Arming

Workstation: `python3 -B tools/arm-publisher-follow.py` with the usual M-U2 lab environment
and the private Cloudflare files. Re-apply without new replicas:
`python3 -B tools/arm-publisher-follow.py --refresh`. Evidence: the directory
named by `PODMESH_FOLLOW_STATE_DIR` (default `~/.podmesh-publisher-follow`). `--lease` on `ha-standby.py rotate` overrides the
ledger's previous policy (a 20 s leftover from a suite must not starve a standing demo).
