# Migration integration: boundaries before implementation

Status: implementation plan. Only the source-side preparation and checkpoint phase exists, experimentally, in the development tree (not packaged); transfer, destination preparation, restore, source exclusion, reservation release and placement commit are not implemented.

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

For product acceptance, the transport/controller may use SSH initially to reach a remote PodMesh API client, but mutations must be requested through explicit PodMesh operations. Direct Podman reads remain independent verification. A later authenticated network API must preserve these contracts. Authorized direct Podman/CRIU probes may investigate feasibility separately; their success does not satisfy API acceptance.

## Implemented source phase (development, not packaged)

This is not the kit. The kit checkpoints a privileged outer container in its own VFS store and reconciles inner Podman metadata for one counter workload. PodMesh checkpoints a network-disabled, mount-free, journal-owned container in the **default rootful overlay store**, with no reconciliation. It shares only the packaged runtime: `podmesh-vzcriu` (binary SHA-256 pinned) selected through the `podmesh-vzcriu-helpers-node` private `criu` shim placed first on Podman's `PATH`. The distribution CRIU and `/opt/vzcriu-kit` are untouched. A default-store Alpine checkpoint with this runtime was observed to succeed on the lab; a subsequent operator-supplied Claude report records successful direct Podman restore of a default-store Alpine fixture. This is a feasibility probe, not a destination API implementation or independent verification by this documentation review. See [experimental scope](EXPERIMENTAL-SCOPE.md) for its conditions and evidence gaps.

The local API contract is in LOCAL-API.md. Design facts established by lab probes:

- Only musl-based processes are accepted. CRIU 3.15 predates rseq support and glibc registers rseq by default; this is detected read-only from process mappings before any suspension.
- Killing the process tree running `podman container checkpoint` during the CRIU dump destroyed the application without producing an archive. The checkpoint therefore runs in its own transient systemd scope, with output written to files, so that a service crash or restart does not interrupt it; a retry then finalizes the completed checkpoint without capturing again. This does not protect against CRIU, Podman or host failure.
- The reservation is durable before suspension and blocks generic create, start, delete and clone. It is never released automatically.

## Gaps requiring the reviewed protocol

- **Release/recovery:** releasing a reservation would let the source run again. Checkpointing without `--leave-running` ends the process, so a local "recovery" can only start the application fresh (losing its memory state) or restore from the archive, and a restore elsewhere may already have happened outside PodMesh. PodMesh records no transfer authorizations and cannot prove their absence. Which proof is sufficient, who may issue it, and whether local recovery restores or restarts are design decisions; no release operation exists.
- **Transfer and destination:** no copy, destination preflight, image availability check, restore or placement report exists. `destination_host_uuid` is recorded, not verified or contacted.
- **Exclusion:** a stopped, checkpointed source with a local reservation is not fencing. A direct administrator `podman start` bypasses PodMesh.
- **Restorability:** a direct default-store restore is reported for one restricted Alpine fixture; destination API recovery and broader kernel, runtime and image compatibility remain unqualified.

## Evidence scope

A successful test of the nested counter proves that workload and runtime combination only. It does not validate arbitrary external volumes, networking, live TCP sessions, application-level exactly-once effects, continuous memory replication or automatic HA. Each needs a separate acceptance case.
