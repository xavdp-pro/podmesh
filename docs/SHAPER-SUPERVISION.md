# Integration with Shaper's supervision loop

Status: architecture contract; end-to-end integration not yet qualified.

The common definition lives in
[Shaper's fractal observation and parent-led correction](https://github.com/xavdp-pro/shaper-three-layers/blob/main/40-TRANSVERSAL/10_HELM_GOVERNOR_MAKER_MAPPING.md#fractal-observation-and-parent-led-correction).
PodMesh supplies local operations and evidence to that loop; it does not introduce
another governor or supervisor hierarchy.

In ShaperOS mode, the integration forwards operation identity, universe and host
identity, dated observations, failures and verification references into Logger.
Historical results remain distinguishable from fresh observations. The maker
invokes permitted PodMesh operations; the governor retains desired state and
coordination. The parent checks the child's outcomes through its authorized
observation surface. A successful API response alone is not proof of repair.

A missing or stale observation, an unreachable host, or an incomplete checkpoint
must remain explicit. Neither logs, UUID/IP mappings nor replicated manager state
authorize duplicate activation. Structural changes to an observer's own active
infrastructure require the external parent/guardian path defined by Shaper.

Standalone PodMesh keeps its local journal and API without requiring ShaperOS or
Logger. The adapter, parent-child evidence flow and complete correction loop still
need implementation review and functional tests; existing local lifecycle tests
do not prove this integration. Qualify the common definition's failure scenarios
through the installed interfaces before claiming it operational.
