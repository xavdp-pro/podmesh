# Experimental4 qualification

Status: live lab tests and independent counter-review completed for the scope below.

The local API now supports explicit start and stop of journal-owned containers.
Start reports the observed application state. Stop requires a timeout and an
explicit escalation policy. Verified retries return historical results together
with a fresh observation. Deletion checks the journal's container identity rather
than trusting labels alone.

## Observed evidence

On each of three Debian 13 amd64 hosts, the installed package passed six suites:
installation (7 checks), lifecycle (6), deletion ownership (13), start/stop (40),
clone (25), and interrupted clone (9). A separate package rehearsal passed on one
host: removal, reinstallation, rollback to experimental3 from a hash-checked local
archive, and re-upgrade preserved host identity, journal and a running workload.
Pre-existing containers, images and volumes remained unchanged in the recorded
before/after comparisons. Temporary snapshot directories were empty afterward.

The experimental4 package downloaded from the public repository matches the
build artifact, SHA-256:
`44d672c17591181729048e97f4ca25acaa90177b9aaa336c78604885f62e52bf`.
Raw lab logs remain outside the public Git repository.

## Defects exposed by testing

Podman inherited systemd's `INVOCATION_ID`, keeping conmon within the PodMesh
service cgroup. Restarting the service disrupted started workloads. PodMesh now
removes that environment variable for Podman subprocesses; tests verify the
separate conmon scope and continued execution after service restart.

An interrupted Podman stop can leave the container marked `stopping` while its
application remains alive. This state must not be reported as a completed stop.
The API reports the observed state and records interrupted attempts. Timestamp
checks conservatively refuse retries against a newer execution.

## Boundaries

These are network-disabled Alpine fixtures without external volumes, using the
default rootful Podman store and a root-only local socket. The results do not
prove rootless operation, remote authorization, integrated ShaperOS deployment,
HA, host reboot recovery, migration through the service, or simultaneous external
administrative changes. Rollback was tested on one existing host, not all hosts
or a fresh installation. Purge was not tested.

`authorization_ref` is provenance, not a credential. Recorded container state
is not a fencing mechanism or proof that an unreachable host has stopped.

## Three review passes

1. Governance: the local root-only authority boundary and the separation from
   governor/maker responsibilities remain explicit; migration and HA are not
   claimed by this release.
2. Operator use: API outcomes distinguish historical results from fresh
   observations; package recovery preserves identity and running workloads in
   the tested rehearsal. Remaining deployment modes stay unchecked.
3. Technical/adversarial: Claude Code Opus 5 implemented and exercised the
   changes; Codex inspected source, artifacts and persisted evidence; a separate
   Claude Code Sonnet 5 review found no blocking defect within this scope.

The independent review flagged the version-dependent Podman stderr warning used
for escalation detection. Keep this limitation visible: exit code 137 alone
cannot prove that PodMesh forced the stop, so it must not replace attribution
with an unconditional inference. Future Podman versions require requalification.
