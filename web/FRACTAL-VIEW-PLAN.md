# Fractal forest view

Requested by Xavier during the nested-container explorer implementation.

## User intent

Provide a separate Fractals view containing multiple independent trees. Each fractal has a logical root (the user's “grandparent”), child universes and descendants. Physical placement can span hosts; moving a universe must not change its logical parentage.

## Presentation

- Searchable fractal selector, including an all-fractals forest view.
- Root cards with expandable/collapsible descendant branches.
- Each node shows name, UUID, role, observed state and current host.
- Filters: fractal, name/UUID, role, host and state. Preserve ancestor context for matching descendants; display ancestors retained only for context distinctly.
- Node selection opens its universe details and permitted actions.
- Root, parent and sibling navigation; explicit loading, stale, unavailable and partial-tree states.
- Mark missing parents, cycles and conflicting relationships rather than silently inventing or reparenting nodes.

## Required source contract

Authoritative relationship records require fractal UUID, universe UUID, parent UUID (null for an actual root), human name, role, revision/provenance and observation time. Runtime placement and health are separately sourced observations.

Physical Podman nesting is not authoritative Shaper parentage. A nested container list must not be rendered as a verified logical fractal. Until the relationship API exists, this view must show its missing dependency rather than fabricate a forest from names or host placement.

## Validation

Use fixtures with multiple roots, descendants on different hosts, a migrated node retaining its parent, missing/offline parents, conflicting edges and cycles. Verify filters keep ancestor context and clearing search preserves the selected fractal/node. Then qualify the same behavior against the real relationship API.

Status: the read-only console view and pure relationship model are implemented. The local gateway can optionally read a bounded, loopback-only manager relationship endpoint through a same-origin session route. Missing configuration, failed reads, malformed or over-limit envelopes, stale source data and conflicting records remain explicit; observed inventories remain listed separately. Qualification against a real control-services relationship API remains open.

## Per-fractal statistics

Add a root summary and per-branch drill-down: universe count, observed lifecycle/health states, host distribution, CPU utilization, memory usage/limits and storage usage. Keep service health distinct from a running container process.

Aggregation requires membership from the logical relationship registry, timestamped observations, explicit units and sample windows. Container hierarchy counters can include descendants: never sum a parent's inclusive cgroup usage and the same children's usage. Sum non-overlapping accounting domains or show parent-inclusive versus child breakdown separately. Shared image layers, thin snapshots and shared volumes likewise cannot be naively added as independent physical usage.

Show measurement coverage, sample age and unavailable contributors beside totals. Missing or stale observations are not zero; mark incomplete totals partial. CPU percentages require normalization to CPU count and compatible sample windows. Configured limits and actual use are separate series. Fractal totals must not include unrelated workloads on the same host.

Test overlapping parent/child accounting, shared disks, multi-host membership, migration overlap, stale samples and partial host loss before presenting totals as complete.

The implemented aggregation counts each universe UUID once and each explicit accounting scope once. Missing, stale, future-dated, invalid or overlapping measurements make the result partial; incompatible CPU sample windows are not summed, and a wholly absent measurement is unknown rather than zero. Current fixture tests cover cross-host trees, ancestor context under filters, missing parents, conflicting records, visible cycles, shared accounting scopes, filtered totals, stale samples, incompatible CPU windows, malformed universe UUIDs, duplicate fractal names and bounded-input degradation. Live resource aggregation remains unavailable until the manager supplies relationship and per-universe accounting records.
