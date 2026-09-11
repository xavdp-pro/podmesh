# Migration integration: boundaries before implementation

Status: implementation plan, not an available API.

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

The transport/controller may use SSH initially to reach a remote PodMesh API client, but mutations must be requested through explicit PodMesh operations. Direct Podman reads remain independent verification. A later authenticated network API must preserve these contracts.

## Evidence scope

A successful test of the nested counter proves that workload and runtime combination only. It does not validate arbitrary external volumes, networking, live TCP sessions, application-level exactly-once effects, continuous memory replication or automatic HA. Each needs a separate acceptance case.
