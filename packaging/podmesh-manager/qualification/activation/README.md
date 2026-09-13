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
commitments.

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
control socket and systemd Result=success, ExecMainCode=exited,
ExecMainStatus=0. It then removes only the regular fragment whose SHA-256 is
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

The comparator requires four captures for every host:

1. pre-activation: manager disabled, inactive and absent, with the reviewed G2
   configuration already installed and an empty first-use state directory.
2. active-baseline: the exact manager process, TCP listener, control socket,
   effective systemd policy and initial canonical store inspection after typed
   readiness.
3. converged: a separate capture after one owned observation was admitted on
   every replica and the canonical stores converged.
4. post-cleanup: canonical inspection after the typed graceful shutdown and
   exact fragment removal.

Convergence is derived only from stages 3 and 4. The three canonical history
digests must agree, contain at least three facts, have zero incomplete attempts,
and remain converged after all residents stop. Equal diagnostic status counters
do not prove convergence.

The comparator also proves the same package and topology, distinct local host
and replica commitments, reciprocal peer and pair-key commitments, one stable
manager invocation per active campaign, exact state/runtime/socket ownership
and modes, and unchanged existing PodMesh services, rootful containers, routes
and nftables commitments.

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
parsing it.
