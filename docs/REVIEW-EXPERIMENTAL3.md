# Experimental 3 review

Scope: standalone Debian 13 amd64 service, stopped-container cloning without mounts, signed package installation on three lab hosts. Not a product-wide acceptance verdict.

## Governance

The local root-only API remains the execution boundary. Governor/maker authority integration is not implemented. Authorization references remain provenance rather than independently checked credentials. Tests invoke mutations through PodMesh and use direct Podman operations for fixtures and independent observation. No unrelated workload changes are authorized.

## Human and operational use

The package advertises its actual capabilities and experimental scope. Host identity is retained. Completed request replay is historical evidence, not fresh state. Public instructions describe generic Linux hosts and do not require the laboratory hypervisor. Standalone package removal retains state; operators must know this before reuse.

## Runtime and adverse conditions

Reviewed changes add source provenance validation, refusal of running/paused or mounted sources, snapshot provenance, retry reuse and service-owned temporary-file cleanup. The tests cover source/target conflicts, copied contents, independent writes, repeated operations and a service kill during a 384 MiB clone commit followed by retry. All four suites reported PASS on each of three hosts: installation foundations, lifecycle, cloning and interrupted cloning.

Codex independently inspected the source and test scripts, checked the persisted suite results for all hosts, queried the installed API/package on the build host and downloaded the public package. Its SHA-256 matched the publication evidence:

`bf9bc885d36aa7e5c2b7905462dadc8bfb1edda69a0827cfc64aab4b229cf036`

Claude Code performed implementation and initial runtime testing; Codex performed this second review. This is not a separate external security audit. Raw lab evidence is retained outside public Git.

## Verdict

COHERENT WITH CORRECTIONS for this narrow experimental milestone. No claim of general lifecycle completeness, migration, HA, network continuity, volume cloning or ShaperOS-integrated operation. The daemon serializes requests and assumes one service instance per state directory. Host loss and concurrent administrative mutation are not exhaustively validated. Next: explicit start/stop API and their acceptance tests, then migration integration.

Final report reconciliation: Claude's completed report confirms all test exits and unchanged pre-existing containers, images and volumes. Additional limits: the real interruption occurred during commit, not between commit and creation; snapshot reuse in that latter window was simulated. Concurrent administrative writes to a stopped source are not detected. Deletion currently trusts target labels rather than a journal/container-ID binding, and must remain restricted to the local administrator trust domain. Tightening that ownership check is assigned to the next lifecycle milestone before broader use. Package rollback and reinstall are still untested. The final documentation changes were reviewed after the implementation report; this verdict applies to the explicitly limited experimental release.
