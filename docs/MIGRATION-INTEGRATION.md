# Migration integration: boundaries before implementation

Status: partially implemented. The source preparation and checkpoint, the transfer authorization, the destination preflight and restore, the completion and the source retirement exist experimentally in the development tree (not packaged), exercised forward and back between two lab hosts for one workload shape. Reservation release, abandonment, local restore and placement commit are not implemented. LOCAL-API.md and MIGRATION-PROTOCOL.md describe what exists; this page keeps the boundaries, the evidence scope and the remaining gaps.

The published reproduction kit uses its own VFS store and a fixed nested counter workload. The initial PodMesh daemon uses the default rootful store. Wrapping the kit and calling that generic migration would hide this mismatch.

## Required separation

1. Keep the current generic lifecycle backend and an explicitly named experimental nested-lab migration backend distinct.
2. Identify the source host, destination host, universe and runtime store explicitly. Never look up a universe by a name alone across different stores.
3. Package the patched CRIU binary under its own path. Do not silently replace the distribution CRIU executable.
4. Preserve the workload-specific runtime metadata reconciliation as an experimental adapter, not a generic repair algorithm.

## Local API phases

- Prepare destination: verify runtime compatibility, image availability, free space and absent target before suspending source.
- Checkpoint source: persist operation state and artifact metadata, verify source stopped.
- Transfer: copy artifacts with content hashes; retry transfer without recapturing a valid archive.
- Restore destination: require verified source exclusion, compatible manifest and intact artifacts.
- Verify: original in-memory marker, progressing counter, usable inner control commands and absence of an active source.
- Commit placement: only report the resulting facts to the maker/governor.
- Recover failure: preserve artifacts and diagnostics. Never automatically restart the source if a destination may have resumed without first excluding it.

Prepare destination, checkpoint source, transfer, restore destination and verify are implemented as described in LOCAL-API.md and MIGRATION-PROTOCOL.md. Commit placement and the recovery paths of a held reservation remain plans.

For product acceptance, the transport/controller may use SSH initially to reach a remote PodMesh API client, but mutations must be requested through explicit PodMesh operations. Direct Podman reads remain independent verification. A later authenticated network API must preserve these contracts. Authorized direct Podman/CRIU probes may investigate feasibility separately; their success does not satisfy API acceptance.

## Implemented source phase (development, not packaged)

This is not the kit. The kit checkpoints a privileged outer container in its own VFS store and reconciles inner Podman metadata for one counter workload. PodMesh checkpoints a network-disabled, mount-free, journal-owned container in the **default rootful overlay store**, with no reconciliation. It shares only the packaged runtime: `podmesh-vzcriu` (binary SHA-256 pinned) selected through the `podmesh-vzcriu-helpers-node` private `criu` shim placed first on Podman's `PATH`. The distribution CRIU and `/opt/vzcriu-kit` are untouched. A default-store Alpine checkpoint with this runtime was observed to succeed on the lab, and the destination phase is now implemented through the API and exercised forward and back between two lab hosts, with memory continuity observed from outside the universe. The earlier direct-Podman restore stays a labelled feasibility probe, not an API result. See [experimental scope](EXPERIMENTAL-SCOPE.md) for the conditions under which probes are run and reported.

The local API contract is in LOCAL-API.md. Design facts established by lab probes:

- Only musl-based processes are accepted. CRIU 3.15 predates rseq support and glibc registers rseq by default; this is detected read-only from process mappings before any suspension.
- Killing the process tree running `podman container checkpoint` during the CRIU dump destroyed the application without producing an archive. The checkpoint therefore runs in its own transient systemd scope, with output written to files, so that a service crash or restart does not interrupt it; a retry then finalizes the completed checkpoint without capturing again. This does not protect against CRIU, Podman or host failure.
- The reservation is durable before suspension and blocks generic create, start, delete and clone. It is never released automatically.

## Gaps requiring the reviewed protocol

- **Release/recovery:** releasing a reservation would let the source run again. Checkpointing without `--leave-running` ends the process, so a local "recovery" can only start the application fresh (losing its memory state) or restore from the archive, and a restore elsewhere may already have happened outside PodMesh. PodMesh records no transfer authorizations and cannot prove their absence. Which proof is sufficient, who may issue it, and whether local recovery restores or restarts are design decisions; no release operation exists.
- **Placement report:** the destination verifies that the handoff names it before acting, but no maker or governor is contacted and no placement fact is reported upward. The two hosts never talk to each other: documents and archives are carried by the requester's controller.
- **Failed-restore cleanup:** a restore that fails after Podman started can leave conmon and CRIU processes that PodMesh did not start; the abort reports them and never kills them. The policy, a cgroup-scoped detection and a verified reclaim are an open operator decision.
- **Exclusion:** a stopped, checkpointed source with a local reservation is not fencing. A direct administrator `podman start` bypasses PodMesh.
- **Restorability:** qualified for one shape only — Alpine, musl, network-disabled, mount-free, about 0.5 GiB — on two lab virtual machines with identical kernel, runtime hashes and image identity, forward and back. Larger or glibc workloads, other kernels or runtimes, networking, volumes, packaging and any third host remain unqualified. Migration downtime was not measured.

## Evidence scope

A successful test of the nested counter proves that workload and runtime combination only. It does not validate arbitrary external volumes, networking, live TCP sessions, application-level exactly-once effects, continuous memory replication or automatic HA. Each needs a separate acceptance case.
