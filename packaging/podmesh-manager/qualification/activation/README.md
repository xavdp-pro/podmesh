# Manager2 live activation evidence harness

This directory contains the fail-closed harness for the first authenticated,
three-host activation of the exact installed podmesh-manager manager2
candidate. It qualifies bounded replication of non-exclusive observations. It
does not qualify HA, fencing, DNS, routing, Podman effects, automatic failover,
or production use.

Every command runs locally on one disposable laboratory host. The operator
orchestrates the three hosts through an external, reviewed campaign. Private
addresses, peer keys, complete configurations and the campaign salt remain
outside Git. Published evidence contains only stable aliases and salted
commitments. The label of an identity the comparator joins names the kind of
value — `replica-id`, `logical-manager-id`, `endpoint` — never the place it was
observed, so that one identity can be joined across configuration, peers,
listeners and inspection without ever seeing the value. That join also requires
each replica to bind exactly the endpoint its peers advertise for it: a loopback
or wildcard bind fails the comparison.

## Boundaries

The manager package remains disabled and has Restart=no. The activation
fragment permits exactly two literal IPv4 peers:

~~~ini
[Service]
Environment=PODMESH_MANAGER_NETWORK_MODE=authenticated-static-peers
RestrictAddressFamilies=AF_UNIX AF_INET
IPAddressAllow=<peer-1>/32
IPAddressAllow=<peer-2>/32
~~~

The packaged IPAddressDeny=any remains in force. The collector checks the
effective systemd properties, not only the fragment text, and commits them
without disclosing addresses. HMAC peer authentication remains the application
security boundary: the configured systemd IP policy is not described as
enforced unless a separate platform-specific behavioral test proves its cgroup
filter.

activate-host.sh writes a checksummed ledger before the first systemd mutation,
installs only 90-g2-network.conf, starts only the manager, waits for the exact
packaged MainPID and typed status response, then captures an active baseline. It
never enables the unit.

Activation and configuration transition share the root-only
`/run/podmesh-manager-qualification/manager2.lock`. Immediately before the
first systemd start, activation atomically writes and fsyncs
`/etc/podmesh-manager/.manager2-activation-started`. Cleanup retains that marker.
It permanently closes the configuration rollback window even if database, lock
and socket artifacts are later absent. Direct root execution that bypasses this
harness and its shared lock is outside the qualification model.

Rollback sends the private typed request {"operation":"shutdown"}. It requires
the exact acknowledgement, successful natural process exit, removal of the
control socket and systemd `Result=success` plus `ExecMainStatus=0`. Depending
on the systemd version and inactive-unit metadata retention, `ExecMainCode` may
be reported as `exited`, `1` or `0`; the typed acknowledgement, PID
disappearance and successful unit result remain mandatory. It then removes
only the regular fragment whose SHA-256 is
bound in the ledger. Failure to prove graceful shutdown leaves the fragment and
evidence in place; it never converts a forced signal into a clean result.

If `systemctl start` itself returns failure, the ledger records a distinct
`start-failed` state. Cleanup then proves that no packaged manager process is
left, removes only the hash-bound drop-in, resets the failed unit state and
emits `RECOVERED_NOT_QUALIFIED`. That recovery never creates activation or
convergence evidence. An unrecorded later crash remains fail-closed and requires
incident analysis rather than being reclassified as a failed start.

The durable store and protected configuration are never removed. Configuration
transition is a separate prerequisite under config-transition/.

## Four evidence stages

### Version contract

New captures use `podmesh-manager-live-activation-evidence/v3`. A present
privacy-preserving `inspection` projection uses integer schema version `4`; the frozen
candidate input inspected by the collector remains private inspection version `3`.
The matching comparator output is
`podmesh-manager-live-activation-comparison/v3`. The activation seal refuses a
converged file or active baseline carrying another evidence version, and the comparator
refuses evidence `/v2`, published inspection `3`, and comparison `/v2` rather than
interpreting old and new shapes under one name. A `/v2` evidence object is rejected by
the exact-shape check before the version check, so the error is the generic `unsafe
shape`; the rejection is real, although its diagnostic does not name the unsupported
version.

Historical `/v2` captures are immutable. Reproduce their stored comparison with the
activation comparator at commit `9d814b2`, or capture preserved stores again as new `/v3`
evidence with new checksums. Never rewrite a `/v2` file to claim `/v3` compatibility.

The comparator requires four captures for every host:

1. pre-activation: manager disabled, inactive and absent, with the reviewed G2
   configuration already installed, manager process count zero, and a typed inspection
   of either an absent fresh store or a present durable store containing retained debt.
2. active-baseline: the exact manager process, TCP listener, control socket,
   effective systemd policy and initial canonical store inspection after typed
   readiness.
3. converged: a separate capture after one owned observation was admitted on
   every replica and the canonical stores converged.
4. post-cleanup: canonical inspection after the typed graceful shutdown and
   exact fragment removal.

Convergence is derived only from stages 3 and 4. The three canonical history
digests must agree, contain at least three facts, and remain converged after all
residents stop. The contract retains and classifies incomplete attempts by
identity as pre-existing, terminal, accounted, or unaccounted; G2 requires zero
**unaccounted** new incomplete attempts. It never requires deletion or fabrication of a terminal
row. Corroboration is direction-aware: inbound and outbound attempts bind to their own
valid non-terminal folded rows, while a new strand still open at post-cleanup is reported
as unaccounted. Equal diagnostic status counters do not prove convergence.

A G2 PASS is an exchange-accounting result over the captured manager processes. It
must report the full attempt vocabulary, the conditions the published evidence could
not decide, and its collector-honest trust model. It does not prove takeover, fencing,
exclusive activation, DNS recovery, host-loss recovery, long-running viability, or
manager high availability. Those remain later gates even when G2 passes.

The comparator also proves the same package and topology, distinct local host
and replica commitments, reciprocal peer and pair-key commitments, one stable
manager invocation per active campaign, exact state/runtime/socket ownership
and modes, and unchanged existing PodMesh services, rootful containers, routes
and nftables commitments.


## Independent-review closure and remaining boundary

The fifth review findings N1-N7 are covered by the offline regression suites. Corroboration is
direction-aware; the fifteen non-equivalent mutations named by the review are caught by their
named unit or CLI tests;
pre-activation debt remains byte-identical; replay evidence is labelled `replay` when a
matching sender retry exists and `receiver_asserted` when only the receiver retained the
replay; the unreachable all-replica branch is absent; pre-activation quiescence is checked
again after inspection; and row identities and shapes are frozen across stages.

More than one valid replay of the same operation is legitimate. Any fully joined replay
is sufficient for the `replay` branch. A receiver-only replay may satisfy the weaker
`receiver_asserted` branch under the published collector-honest trust model. Inbound rows
that appear only after the converged capture and have no matching sender are reported in
`unmatched_inbound_rows` rather than silently strengthening the result.

This closes the offline gate only. A real evidence-v3 campaign and preserved-store
derivation across at least two hosts remain required before G2 can be qualified.

## Per-host activation

Use the same root-owned mode 0600, at-least-32-byte salt on all three hosts.
Keep it outside the evidence tree.

~~~sh
activation/activate-host.sh \
  --host-alias lab-a \
  --salt-file /private/manager2-activation-salt \
  --candidate-verification /private/manager2-verification.json \
  --dropin-source /private/lab-a-manager-network.conf \
  --evidence-directory /private/evidence/manager2-live/lab-a
~~~

Repeat in the frozen host order. Do not inject traffic until all three
active-baseline.json captures succeed.

## Bounded observation and convergence

Each replica owns one exact scope:

- g2/lab-a/observations
- g2/lab-b/observations
- g2/lab-c/observations

Submit one fresh operation through the private local socket on its owning host:

~~~sh
activation/append-observation.py \
  --operation-id <frozen-unique-operation-id> \
  --scope g2/lab-a/observations \
  --subject campaign-probe \
  --value <frozen-non-secret-value>
~~~

Busy or uncertain responses retry the identical serialized request. A changed
operation needs a new operation ID. After the external campaign has observed
stable canonical equality, capture each host separately:

~~~sh
activation/capture-host.sh \
  --host-alias lab-a \
  --stage converged \
  --salt-file /private/manager2-activation-salt \
  --candidate-verification /private/manager2-verification.json \
  --output /private/evidence/manager2-live/lab-a/converged.json \
  --with-inspection
~~~

After that capture passes local inspection, bind its checksum into the
resumable activation ledger:

~~~sh
activation/activate-host.sh \
  --mode seal-converged \
  --host-alias lab-a \
  --salt-file /private/manager2-activation-salt \
  --candidate-verification /private/manager2-verification.json \
  --evidence-directory /private/evidence/manager2-live/lab-a
~~~

## Resumable cleanup

~~~sh
activation/activate-host.sh \
  --mode rollback \
  --host-alias lab-a \
  --salt-file /private/manager2-activation-salt \
  --candidate-verification /private/manager2-verification.json \
  --evidence-directory /private/evidence/manager2-live/lab-a
~~~

The ledger records each completed mutation. Repeating cleanup after a later
capture failure resumes from the retained state. A completed ledger is
idempotent. Ledger files and their sidecars are fsynced independently; after a
crash between the two renames, rollback repairs the sidecar only after the
root-owned ledger passes the exact host, candidate, schema and state checks.
A hash-owned temporary drop-in left before its rename is removed by the
prepared-state rollback. The persistent activation marker is bound to the host
alias, package version and exact binary hash. Use a fresh evidence directory
for another campaign with the same candidate.

`resume-cleanup` is an explicit laboratory recovery for a resident that already
stopped while a cleanup verifier refused to certify the exit. It requires an
active or converged ledger, a fully inactive unit and the exact retained
drop-in. Under the shared lock it starts the same candidate, proves typed
readiness, immediately requests typed shutdown, records the extra cleanup-only
invocation and resumes hash-bound removal. This recovery must be disclosed in
the campaign result and never strengthens the activation or convergence claim.

## Comparison

~~~sh
activation/compare-evidence.py --phase three-host \
  --pre /private/evidence/manager2-live/*/pre-activation.json \
  --active-baseline /private/evidence/manager2-live/*/active-baseline.json \
  --converged /private/evidence/manager2-live/*/converged.json \
  --cleanup /private/evidence/manager2-live/*/post-cleanup.json
~~~

The result always carries ha_claim: "absent". Run
activation/tests/run-tests.sh before transferring the harness. Its tests are
offline and synthetic; successful real-host evidence remains mandatory.
The comparator verifies the SHA-256 sidecar beside every evidence input before
parsing it. For an active stage it also requires the drop-in hash that
validate-dropin.py computed over the bytes it parsed to equal the hash
capture-host.sh took of the installed file: the validated grammar and the
installed policy must be one file, and an absent drop-in carries no such hash.
