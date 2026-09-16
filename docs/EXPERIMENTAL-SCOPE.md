# PodMesh experimental scope and review contract

Status: operator-requested clarification, 2026-09-11. This records the current
research direction; it is neither a SHAPER canon amendment nor production qualification.

## What we are building now

Xavier de Poorter is exploring virtualization-like operational capabilities using
Podman: portable workload identity, cloning, memory-preserving migration, recovery,
and eventually replicated control services across autonomous Linux hosts. Nested
Podman is an intentional research subject. PodMesh is an independent project,
usable without SHAPER OS, and a candidate integration mechanism for SHAPER.
Containers share a host kernel; these experiments do not turn them into virtual
machines or establish equivalent isolation, hardware support or fault tolerance.

The immediate goal is to turn successful bounded experiments into reproducible,
agent-accessible API operations, packages and externally checked results. Technical
limitations should lead to measured experiments and durable fixes, not repeated
planning or a return to another runtime merely because it is already documented.

## Current standard versus candidate mechanism

SHAPER V1.14 Rule 11, amended on 16 September 2026 by operator decision, admits
two universe shapes — `lxc` and `nested` (a rootful Podman container carrying
its own Podman) — and a fourth host family `nested`. Docker is never a runtime.
PodMesh is named there as the optional manager of `nested` universes, never as a
source of authority. The amendment adopts the shape; it does not qualify PodMesh.
Every limit in the evidence ladder below still holds, and the canon repeats the
migration bounds rather than extending them. PodMesh's active manager replica is
not a SHAPER governor (Rule 37).

Keep three statements distinct:

1. **Current SHAPER standard:** what a conforming deployment must implement today, in either shape.
2. **PodMesh experiment:** an explicitly identified candidate mechanism tested in
   isolated fixtures under the operator's instructions.
3. **Future integration:** requires a reviewed proposal explaining which runtime
   requirements change and which guarantees are retained, followed by qualification.

Do not claim an exception for production. The `nested` host family is a Rule 11
token since 16 September 2026; PodMesh itself is a tool, not a family. Host
provisioning and the workload runtime are different dimensions; their eventual
relationship needs an explicit adapter design.

## Responsibilities and invariants

In the SHAPER integration target, the governor maintains declared desired state;
makers retrieve authorized work, invoke local operations and report observed facts.
PodMesh supplies runtime mechanics and evidence. An experimental manager universe
packages control services; it is not another source of authority. Regrouping those
services must preserve the governor/maker separation and local recovery mechanisms.

Preserve stable logical UUIDs, explicit authority, verifiable runtime ownership,
secret boundaries, parent-led structural repair and honest evidence. A destination
runtime container ID may differ after restore: record the verified import in the
destination journal instead of bypassing ownership checks. Replicas do not acquire
permission to activate merely because a peer becomes unreachable.

## Evidence ladder as of this review

| Stage | Evidence and limit |
| --- | --- |
| Nested reproduction kit | Selected nested counter workloads resumed with memory and inner control restored; its store, privilege assumptions and reconciliation are fixture-specific. |
| Packaged lifecycle API | Inventory, creation, start, stop, deletion and restricted cloning have recorded laboratory tests on three hosts. This is not arbitrary-volume or network qualification. |
| Source checkpoint API | Experimental development implementation records a reservation and checkpoint artifact in the default rootful store. This is not a complete migration. |
| Default-store destination probe | The operator supplied a Claude report of successful direct Podman restore after an API checkpoint, using an Alpine, network-disabled, mount-free fixture. The memory marker survived and the counter progressed. This review did not rerun the probe or independently inspect the hosts. |
| End-to-end destination API | Implemented experimentally and qualified only for the recorded two-host fixture: one Alpine, musl, network-disabled, mount-free workload of about 0.5 GiB, matching kernel, runtime hashes and image identity, forward and return, with the handoff verified, ownership imported on the destination and memory continuity observed from outside the universe. Independently counter-reviewed. Recovery of a held reservation, packaging, a third host, networking, storage and HA remain. |
| Networking, general storage, replicated manager and HA | Research targets; no general availability or lossless failover claim follows from the preceding rows. |

The reported destination probe used a placeholder destination identity, direct
Podman restore and a transient systemd scope. Those are disclosed exploration
conditions, not acceptance of the future transfer protocol. The reported restore
command took about 0.864 seconds; this is not total migration time or downtime.
The report lacks a CRIU restore log, so binary selection through PATH is not
independent proof of the actual restore engine. The surviving conmon scope also
needs an explicit lifetime and cleanup contract. Keep raw evidence in the private
laboratory evidence store; publish sanitized, reproducible findings.

## Exploratory probes versus product acceptance

Direct Podman, CRIU and system instrumentation are legitimate tools for authorized
isolated feasibility probes. Label them as such and retain their exact conditions.
They must not count as successful PodMesh API implementation.

Product acceptance exercises mutations through PodMesh's API, with its CLI as a
client. Independent runtime reads check actual effects. A missing API capability
remains missing even when a direct command succeeds. Reuse an intact checkpoint
when investigating restoration unless a demonstrated archive defect requires
recapture. Preserve diagnostics and avoid duplicate activation during recovery.

## Instruction for the continuing implementation agent

Read the intent, this scope, the delivery checklist, migration integration and
provisional migration protocol before implementing the next bounded milestone.
Use the current source and recorded evidence to identify what is really missing.
Continue the authorized Podman research and make its successful mechanisms durable
in code, packages and tests. Report an old runtime restriction as a conformance
boundary, not as a demand to abandon the experiment. Do not weaken ownership,
authority or exclusion checks to make a test pass.

For each finding, distinguish a stale document, a technical failure, an untested
assumption and a proposed change to the SHAPER standard. Explain the smallest fix,
its evidence and remaining limitations. Never merge divergent canon branches,
delete historical copies or edit archived releases as routine documentation hygiene.

The destination adapter has since been qualified through the PodMesh API and the
provisional handoff contract, including invalid artifacts, wrong destination identity,
interruption, retry, imported ownership and source exclusion, on two hosts. A copied
checkpoint and a stopped source still do not prove safe automatic failover.

Next: the recovery paths of a held reservation, and the cleanup of the runtime processes
a failed restore leaves behind. A reclaim is acceptable only with proof of cgroup
membership and a start time at or after the claim, an explicit opt-in, verification from
outside that the cgroups disappeared and the container is absent, a declared disk-recovery
tolerance, and an incomplete-reclaim outcome for any residual process. Who may request
such an action remains an open product policy; a local root socket is not a delegated
authority model.
