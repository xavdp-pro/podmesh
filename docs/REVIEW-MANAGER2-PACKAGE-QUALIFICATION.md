# Manager2 Stage P package qualification review

Date: 2026-09-13  
Source commit: `ff77b1f946e82af421de2ccdbe06d8cd45b70c33`  
Package: `podmesh-manager` `0.1.0~manager2+gff77b1f946e8`  
Scope: deterministic package assembly, signed experimental APT binding,
inactive upgrade, separate configuration transition and default refusal on
three existing laboratory hosts

## Result

Stage P passed for its declared package-only and default-refusal perimeter. Two
assembly runs produced the same Debian package from the exact release
binary associated with commit `ff77b1f946e8`. The signed APT metadata binds that
package, and all three existing laboratory hosts passed the inactive
manager1-to-manager2 upgrade comparison. A later operator-owned configuration
transition added the manager2-required `observation_writer_uid` field while
retaining empty grants. The installed manager2 binary then passed offline
validation and refused default service startup on all three hosts.

This result does not qualify network activation, replication convergence,
takeover, DNS recovery, fencing, restart, rollback, schema-compatible activation
or high availability.

## Candidate and signed publication binding

- Source commit:
  `ff77b1f946e82af421de2ccdbe06d8cd45b70c33`
- Release binary SHA-256:
  `cbd5020a37d2b6b2ab94ee41a7ea9d2f50d0da36c6fb972f795c8a1fe3660128`
- Debian package SHA-256:
  `4e475a2421302b5a3c1c6583853f9e04515076cbd00f430e805ecb9a3ecefc8d`
- Package version: `0.1.0~manager2+gff77b1f946e8`
- APT signing fingerprint:
  `870B13865A81810E19109668A7DE52F62814551B`
- Signed `Packages` index SHA-256:
  `f366282169ae31a4c38a927744fbd11744a8ef7aef4d1095aa3075cb8e4a8a1f`

The retained `candidate-contract.json` binds the source commit, package version,
binary hash, package hash, payload and maintainer scripts. `gpgv` accepts the
retained `InRelease` with the pinned fingerprint; that release binds the exact
`Packages` index, whose manager2 stanza binds the package hash above. Four package
copies in total are retained: two assembly outputs, the top-level publication
candidate and the downloaded package. They are byte-identical.

This is deterministic Debian package assembly from one release binary built from
the exact source commit. The release binary was not independently rebuilt, so
this is not a reproducible Rust compilation claim or an independent proof that
source bytes necessarily produced that binary.

## Inactive three-host upgrade

The frozen inactive-upgrade comparator returned `PASS` for `lab-a`, `lab-b`,
`lab-c` and the combined three-host result. Every pre/post pair binds manager1
`0.1.0~manager1+g5319ab150fc3` to manager2
`0.1.0~manager2+gff77b1f946e8`, with clean `dpkg --verify` evidence and the
expected binary hash for each candidate.

Across the upgrade window on each host:

- the manager unit remained loaded, disabled and inactive, with no process,
  runtime directory, invocation or restart;
- the protected configuration and the complete state-tree commitment remained
  unchanged;
- the manager account and empty state directory remained unchanged;
- the lifecycle and observer package, service, PID, invocation and socket
  observations remained unchanged;
- the rootful Podman commitments and host boot commitment remained unchanged.

The upgrade evidence proves the exact installed version and payload. It does not
record APT origin on each host and therefore does not claim host-side APT-origin
proof beyond the separately verified signed candidate binding.

## Separate configuration transition

The package upgrade did not rewrite the protected configuration. After the
inactive upgrade comparison passed, an operator-owned step separately added
`"observation_writer_uid": 0` to each configuration. Public, secret-free records
bind each pre-upgrade configuration commitment to the resulting default-refusal
document commitment and report:

- added keys: `observation_writer_uid`;
- removed keys: none;
- changed keys: none;
- `observation_writer_uid`: `0`;
- grant count: `0`;
- final configuration: regular file, one link, mode `0640`, UID `0`, GID `104`;
- temporary candidate count: `0`;
- protected backup: present, one link, mode `0600`, UID/GID `0`;
- service: disabled and inactive;
- offline validation before and after replacement: passed;
- archived procedure SHA-256:
  `5766017862bd5c09b2140b4c43b2e4a2dd6025831b5048c356294888ecf71669`.

The independent closure review recomputed the commitments, exact added key,
empty grants, metadata and binary binding against the retained private snapshots
without publishing any secret value. It checked the procedure hash against the
archived script. The records prove the resulting content and metadata. Execution
of the archived transition procedure is operator-attested; the later metadata
capture and procedure hash do not by themselves prove which script ran at
transition time.

The public transition records and summary are retained under
`docs/qualification/manager2/`. Their manifest is integrity-checked but unsigned.
The raw configurations, pair keys and salts remain outside the public evidence.

## Three-host default refusal

After the separate transition, the installed manager2 binary validated each
configuration as the dedicated service account. Each attempt to start the
unchanged packaged unit then reached the intended default network refusal. On all
three hosts, systemd recorded `ActiveState=failed`, `Result=exit-code`,
`ExecMainStatus=1`, `MainPID=0`, `NRestarts=0`, a disabled unit and no drop-in.
The InvocationID-bound journal evidence records the expected network-disabled
refusal.

External observations found no manager process, control socket or configured
listener, and the manager state directory remained empty. Route, firewall,
rootful Podman container and existing lifecycle/observer service commitments
remained unchanged. The combined refusal summary and all three host results
returned `PASS` under the frozen harness described below.

The refusal campaign used the qualification harness frozen at source commit
`ff77b1f946e82af421de2ccdbe06d8cd45b70c33`. Its retained bundle has SHA-256
`2a9ebc26b9f0957002c32ea62e4eaa5c3d349ddda2de3ca4abbcd36ad81eb86d`.
That frozen harness did not emit or require `grant_count`; the Stage P zero-grant
claim is established by the separate transition records. The result commit that publishes this review
hardens future campaigns by emitting `grant_count` and refusing any nonzero
count. Its checked-in comparator rejects the older Stage P evidence by design;
reproducing this campaign's refusal `PASS` therefore requires the frozen bundle.

This is deliberate failed-start evidence. It is not resident-service activation
or a running-manager qualification.

## Recovery preparation

Before public repository publication, the operator attested that a
repository-container backup was retained on the approved private NFS target:

- size: `260614993` bytes
- SHA-256:
  `9ebcb1d13128a5908969e9bfa61071f113a00202df597df81b0ee52317e7468b`

The exact private path is retained in the infrastructure repository rather than
this public result. The size and hash identify the operator-attested artifact.
No restore was performed, so this is not restore proof.

## Independent counter-review and evidence limits

Claude Code Opus reviewed the complete Stage P evidence in read-only mode. It
rechecked source and qualification bundle provenance, package identity, signed
metadata, upgrade comparisons, configuration transition, refusal evidence and
privacy boundaries. Its first review raised one important evidence gap: the
public records did not expose the transition and empty-grant relation. After
secret-free transition records were added, the closure review recomputed their
claims against private snapshots and reported no remaining blocker or important
finding for the narrow Stage P claim.

The closure review retained low-severity limits: the public transition statements
about key changes and grant count remain attestations backed by salted
commitments; archived procedure execution remains operator-reported; and the
refusal evidence binds the exact binary indirectly through the immediately prior
upgrade evidence and manager2-only schema field rather than repeating host
package and binary identity in the refusal record.

## Three-pass closeout

- **Governance:** Stage P stayed within signed experimental publication and the
  existing laboratory hosts. The service remained without network authority,
  and secrets stayed outside the public artifacts.
- **Human and organizational:** package installation and operator-owned
  configuration remain separate responsibilities. The disabled default requires
  an explicit later activation decision.
- **Technical and adversarial:** deterministic assembly, signed metadata,
  inactive upgrade, configuration transition and failed-start evidence are bound
  to the declared candidate. Activation, convergence, takeover, DNS, fencing,
  restart, rollback, schema-compatible activation and HA remain open gates.

The Stage P perimeter is `PASS` for the claims above. The larger manager
activation, lifecycle and HA perimeters remain `PARTIAL / OPEN`.
