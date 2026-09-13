# Replicated control-services acceptance plan

Status: executable qualification contract. This document defines evidence and
ordering; it is not evidence that replicated manager HA works.

Owner: Xavier de Poorter, collaborating with OpenAI Codex and Claude Code.

## Purpose

PodMesh has one logical control-services universe with one distinct replica per
host. Each healthy host must retain locally useful control facts without depending
on a shared live filesystem. Replication must preserve history and availability
without allowing two copies to perform the same exclusive effect.

This plan qualifies that design on three Linux hosts. It separates five claims:

1. a replica process restarts from its own durable state;
2. authenticated peers exchange bounded immutable facts;
3. disconnected replicas continue only inside predelegated non-overlapping scope;
4. an external effect gate accepts exactly one current activation epoch;
5. the complete manager, DNS and Maker path recovers after a real host loss.

Passing an earlier claim never implies a later one.

## Required identities and fixtures

- One immutable mesh UUID and one logical control-services UUID.
- Three stable host UUIDs and three distinct replica instance UUIDs.
- Pinned peer credentials and endpoints supplied by local configuration. Received
  data cannot enroll a peer, change a path or widen authority.
- Three non-overlapping disconnected-write scopes and three disjoint address pools.
- One exclusive service name and IP used only by the disposable HA fixture.
- One ordinary non-exclusive observation stream per replica.
- One external observer outside every manager process and one append-only evidence
  directory whose hashes are recorded before cleanup.
- A monotonic test-run UUID, request UUID for every mutation and explicit expected
  failure for every injected fault.

Production workloads, production DNS and undeclared network peers are outside this
plan. Tests use disposable manager state and disposable effects only.

## Evidence contract

Every scenario records:

- exact source commit, package/binary hash, configuration hash and schema version;
- host, replica and logical manager UUIDs;
- wall-clock and monotonic timestamps from the injector and external observer;
- process IDs, exit status and service-manager result;
- sent and received request IDs, byte counts, peer identity and refusal reason;
- pre/post SQLite integrity, event counts, heads, conflicts and receipt counts;
- actual route, DNS answer, socket owner or fixture effect observed outside the
  manager process;
- packet-filter or power-control evidence for the injected fault;
- cleanup result and retained artifacts.

Health endpoints and manager self-reports are diagnostic signals. They do not prove
an exclusive effect, process death, durable commit, DNS publication or recovery.
Missing data is `unknown`; it is never recorded as zero or stopped.

## Global invariants

| ID | Invariant |
| --- | --- |
| HA-I01 | One logical manager may have many replicas, but each replica identity belongs to exactly one declared host. |
| HA-I02 | A received event retains its original identity, producer, sequence, causal relation and digest. |
| HA-I03 | Duplicate, delayed and reordered delivery cannot create another logical event or effect. |
| HA-I04 | A peer cannot enroll itself, select a database path, widen a scope or supply a trusted topology. |
| HA-I05 | Priority selects a coordinator only after reconciliation; it does not select truth or authorize takeover. |
| HA-I06 | Unreachable never means stopped. Remote activation requires externally enforceable exclusion proof. |
| HA-I07 | At most one current epoch can pass the effect gate for an exclusive resource. Older epochs remain refused after restart and reconnection. |
| HA-I08 | Independent predelegated facts merge; contradictory exclusive facts remain retained, visible and blocked. |
| HA-I09 | A failed or refused batch leaves no partial durable import and a retry has one stable outcome. |
| HA-I10 | DNS and routing publish only an externally verified active placement with a current accepted epoch. |
| HA-I11 | Corruption, full storage and incompatible schema fail closed without fabricating success. |
| HA-I12 | Recovery never silently discards an unread incident record, conflict or competing history. |

## Readiness gates

| Gate | Required result | Current state |
| --- | --- | --- |
| G0 Local reducer | Deterministic three-copy reconciliation and conflict quarantine | Qualified in the isolated model |
| G1 Durable process | Crash/restart, concurrent writers, checked facts and receipts | Qualified locally; no host deployment |
| G2 Authenticated exchange | Three real processes, bounded mutual peer authentication and durable imports | Not yet qualified. Two live three-host campaigns of candidate `0.1.0~manager2+gff77b1f946e8` each converged to one canonical digest on all three replicas, at the converged stage and again after typed cleanup, with unchanged unrelated services, containers, routes and firewall. The checked-in comparator still does not PASS: every store retains outbound exchange attempts the candidate deliberately leaves uncertain when a peer at its one-connection incoming limit drops the connection, and the gate requires zero. A reviewed transition then raised `incoming_workers` to 2 on all three hosts and a measurement run showed the accept-and-drop eliminated (`rejected_connections: 0`) with nine strands still appearing, every one of them received, imported and replied to by its peer. The gate's `incomplete_attempt_count == 0` therefore requires that a failure mode the frozen Stage D contract explicitly specifies (`MANAGER-G2-DURABLE-EXCHANGE.md:470`, invariant G2-I07) never occurs during a campaign. Whether that is the right predicate is a specification question for this document's author; separately, the receiver's pre-reply path is O(audit table) and unbounded, which is a real defect. See [FINDING-MANAGER2-LATE-REPLY-STRANDS.md](FINDING-MANAGER2-LATE-REPLY-STRANDS.md) |
| G3 Effect exclusion | Current epoch enforced outside manager memory and old epoch refused | Not yet qualified |
| G4 Host deployment | Signed package, preserved identity, upgrade/rollback and clean install on three hosts | Signed installation, protected three-replica configuration, offline validation and default-disabled refusal pass on three existing hosts; durable identity restart, lifecycle, upgrade/rollback and clean-host requirements remain open |
| G5 Manager service recovery | Real process/host loss, restart and stale return under external observation | Not yet qualified |
| G6 DNS/bootstrap recovery | Resolver and manager recover without circular dependency | Not yet qualified |
| G7 Human-agent operation | Governor/Maker intent, effect and evidence correlate end to end | Not yet qualified |

Scenarios may run only when their prerequisite gate is qualified. A scenario that
needs missing mechanics is prepared, not passed.

## Scenario catalogue

### HA-01 — Normal three-replica convergence

- Prerequisites: G2.
- Setup: start three empty declared replicas; append one independent fact in each
  owned scope; exchange through the configured transport.
- Outside observation: inspect all three stores through separate read-only SQLite
  connections and capture transport request/receipt logs.
- Pass: byte-equivalent logical histories, three distinct replica identities, no
  conflict, no undeclared peer and no effect.
- Data loss: none.
- Cleanup: stop processes and retain all three closed stores.

### HA-02 — Clean replica stop and restart

- Prerequisites: G2.
- Setup: stop one replica cleanly while the other two append owned observations;
  restart it with the same host and replica identity.
- Pass: the stopped replica performs no writes while absent, catches up once,
  preserves receipts and allocates no reused sequence.
- Data loss: none for committed facts.

### HA-03 — Process kill at transaction boundaries

- Prerequisites: G1, repeated after G4.
- Setup: kill before commit, after durable commit before response, during import
  and during receipt replay.
- Pass: uncommitted work disappears; committed work replays once; SQLite integrity
  passes; the external result never contradicts durable state.
- Data loss: only explicitly uncommitted input.

### HA-04 — Real host power loss

- Prerequisites: G3 through G5.
- Setup: cut power to the active fixture host without guest shutdown. Network loss
  alone is not accepted as power-loss evidence.
- Outside observation: hypervisor or switched-power state, effect-gate epoch, route,
  DNS, fixture socket and all surviving stores.
- Pass: the old epoch is excluded before a replacement effect is accepted; one
  replacement becomes observable; the old host cannot reclaim authority on return.
- Allowed data loss: declared recovery-point objective measured in events and time.
- Cleanup: return old host isolated, reconcile, then re-admit it as standby.

### HA-05 — Symmetric network partition

- Prerequisites: G2; exclusive-effect part also requires G3.
- Setup: block all manager traffic between one host and the other two while keeping
  local workloads running.
- Pass: each side writes only inside its predelegated scope; neither side treats
  absence as failure; no standby advertises the exclusive IP; reconnection retains
  and merges independent histories before coordinator selection.
- Data loss: none for accepted local facts.

### HA-06 — One-way partition

- Prerequisites: G2 and G3.
- Setup: allow A to send to B while dropping B to A, then reverse direction.
- Pass: acknowledgements cannot be inferred from outbound success; no half-visible
  transfer grants authority; retries remain idempotent; the effect gate accepts at
  most one epoch.

### HA-07 — Delay, duplication, reorder and truncation

- Prerequisites: G2.
- Setup: proxy transport with deterministic schedules for duplicate requests,
  reversed batches, delayed receipts, truncated frames and response loss.
- Pass: exact events converge once; malformed/truncated frames are refused; no
  partial batch commits; lost responses produce stable retry results.

### HA-08 — Unknown, revoked or impersonated peer

- Prerequisites: G2.
- Setup: wrong credential, right credential with wrong replica ID, revoked key,
  changed endpoint and replay from another mesh.
- Pass: refusal occurs before store mutation; local topology and filesystem paths
  remain unchanged; reason and peer evidence are retained without secret material.

### HA-09 — Stale closed-database return

- Prerequisites: G2.
- Setup: preserve a closed old database, advance the other replicas, then restore
  the old copy under its original identity.
- Pass: it catches up before proposing coordination or allocating a conflicting
  sequence; missing local receipts remain explicit and cannot fabricate replay.
- Data loss: measured and unresolved until the recovery-identity/receipt contract
  is qualified; this scenario cannot qualify automatic activation before then.

### HA-10 — Old active rejoins

- Prerequisites: G3 through G5.
- Setup: isolate the old active, advance the accepted activation epoch elsewhere,
  then reconnect the old process and finally restart its host.
- Pass: the old epoch is refused at the external gate in both cases; its history is
  retained; it synchronizes before becoming eligible for future work.

### HA-11 — Conflicting exclusive claims

- Prerequisites: G2 and G3.
- Setup: inject two validly authenticated but causally competing claims for one
  service IP and two universes.
- Pass: every replica retains both facts, creates the same conflict, blocks the
  affected resource only and continues independent scopes. Priority resolves
  neither claim automatically.

### HA-12 — Clock skew and rollback

- Prerequisites: G2 and G3.
- Setup: advance and retard wall clocks independently, including a backward jump;
  do not alter monotonic test control.
- Pass: wall time affects freshness displays only within the declared tolerance;
  it cannot create a higher epoch, validate a stale permit or reorder causal facts.

### HA-13 — Disk full and SQLite I/O failure

- Prerequisites: G1, repeated after G4.
- Setup: bounded disposable filesystem reaches full capacity; separately inject
  write, fsync and directory failures.
- Pass: requests fail visibly, no half-event/receipt remains, previously committed
  state is readable when the medium permits, and recovery does not report the
  refused operation as committed.

### HA-14 — Corruption and incompatible schema

- Prerequisites: G1.
- Setup: alter facts, receipts and schema version in disposable copies.
- Pass: checksum or schema refusal precedes replay/mutation; no in-band automatic
  repair erases evidence; documented offline recovery is required.

### HA-15 — Bootstrap with and without WireGuard

- Prerequisites: G2 and G4.
- Setup: run the same pinned-peer configuration over directly routed LAN and over
  WireGuard addresses, then remove each path independently.
- Pass: protocol identity and authorization stay identical; WireGuard changes the
  transport path only; loss of either optional path remains explicit.

### HA-16 — DNS/bootstrap circularity

- Prerequisites: G4 through G6.
- Setup: start with manager DNS stopped, one stale resolver cache and one unavailable
  replica. Bootstrap uses pinned seed endpoints independent of manager-served DNS.
- Pass: replicas authenticate and reconcile before publishing DNS; clients receive
  only the verified current service record; expiration and withdrawal are observed.

### HA-17 — Governor, Maker and PodMesh end to end

- Prerequisites: G7.
- Setup: a fictional SaaS Governor authorizes creation and supervision of children
  across three hosts; one parent observes each child through Logger evidence.
- Pass: intent, authorization, local Maker action, PodMesh operation ID, independent
  runtime observation, registry fact and DNS result correlate. A refusal remains a
  refusal throughout the chain.

## Stress campaign

After HA-01 through HA-17 pass once, run a seeded campaign of at least 1,000 bounded
operations per seed across 20 published seeds. Mix 40% independent observations,
20% duplicate/lost replies, 15% process restarts, 10% message reorder/delay, 5%
partitions, 5% storage refusal and 5% exclusive attempts. Every run checks HA-I01
through HA-I12 after each step and after complete reconnection. Preserve the seed,
event schedule, final databases and external effect log for every failure.

Stress success means zero invariant violation and reproducible cleanup. It does not
replace real power-loss, routing, DNS or operational qualification.

## Execution order

1. Freeze peer identity, credential rotation/revocation and protocol version.
2. Qualify G2 with HA-01, HA-02 and HA-05 through HA-08.
3. Freeze recovery identity, receipt restoration and activation-epoch contracts.
4. Qualify G3 with HA-06, HA-10 through HA-12.
5. Package and install on clean disposable hosts; qualify G4.
6. Run HA-03, HA-04, HA-09, HA-13 and HA-14 against installed services.
7. Implement independent bootstrap and DNS publication; run HA-15 and HA-16.
8. Run HA-17, then the seeded stress campaign.
9. Have Claude Code perform an independent counter-review; Codex verifies each
   finding, reruns affected suites and records unresolved limits.
10. Publish an operational assessment with measured recovery time, data loss,
    manual intervention and the exact claims that remain open.

The replicated manager may be called HA only after G0 through G7 and the relevant
real-host scenarios pass. Until then, reports must name the narrower qualified gate.
