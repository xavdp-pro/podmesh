# PodMesh intent

Status: design intent, 2026-09-11. Not implemented SHAPER canon.
Owner: Xavier de Poorter. Prepared with OpenAI Codex — GPT-6 Astra.

## Purpose

Enable human-agent tandems to operate portable Podman universes across autonomous
Linux hosts through explicit intent, bounded operations and verifiable outcomes.
The human brings purpose, judgment and decisions; the agent translates authorized
intent into operations and reports evidence, uncertainty and outcomes. PodMesh is
designed for this collaboration, not for agents acting as autonomous owners.
Human interfaces and agent tools use the same operation and authority contracts.

## Actors and responsibilities

The human defines purpose and authority. The governor maintains desired state.
One maker per host retrieves approved work and invokes PodMesh. PodMesh performs
local technical operations and reports evidence. It does not become a second
governor, infer permissions from prose, or silently change the desired-state ledger.

## Human-agent interaction contract to implement

- Present capabilities, state, expected effects and results in a form the human
  can understand and the agent can process. Preserve human correction and control.
- Discover capabilities, versions, prerequisites and supported operations.
- Receive typed requests with operation ID, target UUID, desired outcome,
  authorization reference and applicable preconditions.
- Inspect current state and the proposed effects before execution.
- Execute within the approved scope; expose durable operation progress.
- Return structured results distinguishing accepted, running, failed and verified
  outcomes, with evidence references and actionable errors.
- Make retry behavior explicit: an operation ID must not duplicate an effect;
  incompatible reuse must fail. Reconcile interrupted work before retrying it.
- Support bounded cancellation where feasible; irreversible phases and recovery
  paths must be explicit. Never claim cancellation rolled back completed effects.
- Preserve provenance, observed-state timestamps and uncertainty.

A natural-language interface may translate an intent into these contracts, but
natural language is not an executable host command or an authority grant.

## Initial operational proof

A fictional SaaS governor requests simple child universes on three hosts. Makers
materialize them through PodMesh; independent observation verifies identity,
placement, progress and parent-child supervision. Extend to migration, replicated
checkpoints, partitions and recovery as capabilities become available. Produce
an operational assessment with failures and manual interventions included.

## Success criteria

Agents can discover, request, observe and safely retry supported operations
without depending on a GUI or undocumented shell procedures. Results prove actual
changes, not only request acceptance. Existing workloads remain autonomous within
the declared partition policy. Exclusivity and data-loss limits remain explicit.

## Boundaries and open decisions

The service, API, UUID/IP allocator and HA coordination remain to be implemented.
Public instructions describe generic hosts. The laboratory is an example, not a
runtime dependency. See README.md for the complete inventory, decisions, candidate
mechanisms, deferred DNS work and qualified evidence. Existing SHAPER governing
texts remain authoritative; this intent does not replace or rewrite them.

## Delivery order: build for our own tandem first

The first users are Xavier and the collaborating agent. Prioritize a working,
observable workflow on our hosts through agent tools and CLI/API. A later human
interface must allow inspection, control and action through those same contracts.
Its absence must not block the initial tandem experiment, and its later addition
must not create a second implementation of operations or authority. Design for
reusable public delivery while validating our own operational needs first.

## Two supported deployment modes to build and validate

PodMesh must support standalone operation on a Linux host without SHAPER OS,
and integrated operation inside a SHAPER OS universe. For our tandem, integration
should reuse existing logging, supervision and authority mechanisms. External
users must not need SHAPER OS to install or operate PodMesh.

Both modes use the same core operation contracts. Provide explicit adapters for
standalone configuration/logging and SHAPER integration rather than duplicate
business logic. A containerized service needs explicitly scoped access to its
managed Podman runtime and required host capabilities; packaging alone does not
grant access. Network and migration privileges, paths and namespace boundaries
must be documented and tested independently in each mode. The integrated mode
is a required target, not a claim that existing host-level code already works
unchanged inside a universe.

## Preferred container distribution

Prefer Alpine Linux for container images wherever the component's dependencies and runtime behavior can be validated. This is a preference, not a requirement to migrate existing Linux hosts or abandon Debian package delivery. Keep exceptions explicit when another base is necessary for compatibility. Validate the actual built binaries, dependencies, logging, supervision, checkpoint and restore behavior for each selected image; a successful Debian-host test does not validate an Alpine image.
