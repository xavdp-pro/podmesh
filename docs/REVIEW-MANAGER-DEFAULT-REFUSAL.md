# Manager identity and default-refusal qualification review

Date: 2026-09-13
Source commit: `5319ab150fc34abb0b1648be0c3a68350975cebb`
Package: `podmesh-manager` `0.1.0~manager1+g5319ab150fc3`
Scope: protected three-replica configuration, pure offline validation and
default-disabled systemd refusal on three existing laboratory hosts

## Result

One logical manager identity was materialized as three host-bound replica
configurations. Each configuration declares the same complete topology, a
different local replica and host identity, the other two replicas as static
peers, and a distinct symmetric key for each replica pair. Only salted
commitments and sanitized aliases entered the final retained evidence;
endpoints, identities and pair keys remain local to the protected host
configurations. Raw operational metadata such as UIDs, process identifiers and
counts is retained where it supports the claim. Scope-grant contents were not
included in the final evidence and receive no claim in this stage.

All three configurations passed the packaged `--validate-config` path as the
dedicated service account. Each attempt to start the unchanged packaged service
then failed on its explicit default network gate. The combined private comparison
returned `PASS`.

This result qualifies configuration shape, local file boundaries, offline
validation and accidental-start refusal. It does not qualify authenticated
exchange, convergence, takeover, DNS recovery or high availability.

## Configuration and validation evidence

The three installed configuration files are owned by
`root:podmesh-manager` with mode `0640`. Independent salted commitment comparison
establishes:

- one equal logical manager identity across three configurations;
- one equal three-replica topology;
- three distinct local replica identities and three distinct host identities;
- exactly two non-local peer entries per replica;
- three distinct pair credentials, each represented symmetrically at both ends.

For each host, offline validation returned:

```json
{"configuration_valid":true,"durable_store_checked":false,"network_started":false}
```

The private runtime directory created for validation remained empty and was
removed. The manager state directory remained empty. External observation found
no database, lock, Unix socket or TCP listener; the validation source also
returns before opening the durable store or binding a transport.

## Default-disabled refusal evidence

Before any systemd override existed, each host attempted to start only
`podmesh-manager.service`. Observation after the process outcome showed the same
result on all three hosts:

- `ActiveState=failed`;
- `Result=exit-code`;
- `ExecMainStatus=1`;
- `MainPID=0`;
- `NRestarts=0`;
- the expected `runtime networking disabled` refusal in the unit journal.

External inspection found no packaged manager process, SQLite database, WAL or
shared-memory file, resident lock, control socket or listener on the declared
manager port. IPv4 route and nftables ruleset commitments were unchanged.
The existing PodMesh lifecycle and web-observer service-state commitments were
unchanged.
The authoritative refusal artifact ends with the manager unit in its expected
failed state. A later reset of systemd's failed-state marker was operational
cleanup outside this qualified evidence and grants no additional claim.

## Evidence handling

The full configuration files and live peer material are not public artifacts.
Private evidence contains configuration and identity commitments, sanitized host
aliases, the declared package version, the packaged unit hash, systemd fields, a
salted journal commitment with a bounded refusal assertion, artifact absence and
before/after infrastructure commitments. Its complete file set has a verified
SHA-256 manifest.

An initial convenience check attempted an unprivileged directory traversal and
received `Permission denied`. It was rejected as evidence. Later root-run JSON
observations independently established the empty state and absent runtime facts
used by this review.

## Remaining limits

- The manager has not yet run with networking enabled on the hosts.
- Static peer authentication has not yet been exercised outside the process lab.
- No durable manager database or identity restart has been qualified on a host.
- The current resident lacks the qualification-only local fact input, canonical
  read-only store verifier and durable per-exchange audit needed to prove G2 on
  hosts; status counters alone are not accepted as exchange evidence.
- No effect gate, Podman action, DNS publication, coordinator selection, fencing
  or takeover has been activated by these replicas.
- Removal, purge, reinstall, upgrade, rollback and clean full-host installation
  remain separate G4 requirements.
- Gate G4 remains partial. Gates G2, G3 and G5 through G7 remain open.

## Independent counter-review

Claude Code Opus, high effort, received the source, exact claim and first private
evidence set in read-only mode. It rejected the initial `PASS` because a dynamic
container projection changed, two firewall observations could have been failed
commands, low-entropy endpoints used unsalted commitments, and the validation
record did not bind the effective UID or temporary runtime lifecycle. It also
requested an InvocationID-bound journal and the effective unit/drop-in state.

Those findings produced the fail-closed refusal harness. The missing firewall
observer was installed before the next test window without adding a rule. A
second independent review then rejected the corrected campaign because the
harness recorded, but did not itself reject, route, firewall and unit drop-in
changes. It also found that the public report described more than the retained
evidence established.

The harness was changed to reject those mutations, its negative tests were
extended, and the three-host campaign was run again with the frozen harness. The
final evidence binds the effective validation UID and temporary runtime
lifecycle, exact unit fragment and drop-in state, InvocationID-bound journals,
stable Podman and existing-service projections, and successful route and
firewall observations. The cross-host comparator independently recomputes the
identity, topology, credential, systemd, journal, route, firewall, service and
Podman relations it accepts. The campaign harness SHA-256 is
`7b765f2a2100ab13061e6c79828bd6a27ea586692f8bebf04737cdb952c2b69b`;
the final comparator SHA-256 is
`f610929332fd4d4d617863a76073c739ba04fbbf3b649657db96d0cd9d0816d0`.
Operational cleanup after evidence capture is recorded separately.

A final Claude Code Opus high-effort review found no remaining blocker in the
code or evidence. Its one remaining important finding was this report's stale
description of the earlier campaign; the final wording and hashes above resolve
that documentation defect. The private evidence directory retains all three
reviews and a complete SHA-256 manifest with owner-only permissions.

## Three-pass closeout

- **Governance:** the standing laboratory mandate covered protected host
  configuration and a reversible failed-start test. No customer service,
  production authority, external effect, secret publication or destructive
  cleanup occurred.
- **Human and organizational:** one logical manager is represented by three
  declared replicas, while each replica and host remains separately attributable.
  The package still grants no authority and cannot start networking by default.
- **Technical and adversarial:** all configurations were validated as the service
  account; cross-host commitments were compared; every unit failed on the intended
  gate; external evidence found no retained manager runtime effect. Replication
  and HA remain explicit later gates.

The configuration and default-refusal perimeter is reconciled against the central
delivery checklist. The larger manager lifecycle and HA perimeters remain open.
