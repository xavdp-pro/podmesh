# PodMesh delivery plan and evidence checklist

Owner: Xavier de Poorter, collaborating with OpenAI Codex and Claude Code.

This is the central delivery tracker. Detailed contracts remain in INTENT.md, RUST-IMPLEMENTATION-PLAN.md, ACCEPTANCE-TEST-PLAN.md, BACKUP-SERVER.md and LVM-LAB-PLAN.md. A checked item records only the scope explicitly stated. Compilation, installation, runtime proof and independent review are separate gates.

## Product requirements

- [ ] Operate for human-agent tandems through typed intentions, API and CLI; human UI later uses the same contracts.
- [ ] Support standalone Linux hosts (physical, VM or VPS) and ShaperOS-integrated/containerized deployment; prefer ShaperOS for our use without requiring it for other users.
- [ ] Preserve governor desired-state ownership and one maker per host; PodMesh executes bounded local operations rather than becoming another governor.
- [ ] Support joining occupied hosts, departure, reconnection and regrouping without replacing existing workload identities.
- [ ] Stable host and universe UUIDs, separate replica identity, human-readable names and explicit operation IDs.
- [ ] Portable universe placement and memory-preserving migration, with verified recovery and one active workload where required.
- [ ] Logical /16, disjoint /24 allocation pools, replicated UUID/IP assignments and stable addresses across movement; actual prefix remains to choose.
- [ ] Support existing routed connectivity and optional WireGuard for control, workload traffic and copies.
- [ ] Common DNS naming, registry/image availability and manager bootstrap without circular dependencies.
- [ ] Local autonomy during partitions within preassigned scopes; reconcile histories before coordination resumes. Priority selects a coordinator, not the most truthful state. Safe remote takeover remains to design and prove.
- [ ] Replicated manager universe per host, with explicit data-loss, exclusivity and recovery contracts.
- [ ] Public reusable code/docs in English; internal infrastructure private; secrets outside Git. French operator dialogue.
- [ ] Signed Debian delivery, reproducible installation, upgrades, removal and restoration.
- [ ] Demonstrate fictional SaaS governor -> manager -> child universes across three hosts and publish an operational assessment with failures and limitations.

## Ordered execution

### 1. Standalone baseline

- [x] Compile initial Rust daemon and CLI on the lab build host.
- [x] Publish experimental Debian package and install through signed APT on three hosts.
- [x] Upgrade all three from experimental1 to experimental2 through APT.
- [x] Verify distinct identities, identity/journal persistence after service restart, inventory against independent Podman output, invalid requests and recovery after errors. Evidence: evidence/baseline-*.json.
- [x] Upgrade all three from experimental2 to 0.1.0~experimental3 through signed APT; host identities preserved; baseline, lifecycle and clone tests re-run on each host. Evidence: evidence/experimental3/. Publishing experimental3 removed experimental2 from the repository pool (one version per suite); rollback relies on the retained .deb in each host's APT cache and is not rehearsed.
- [x] Upgrade all three from experimental3 to 0.1.0~experimental4 through signed APT; host identities preserved; installed, lifecycle, delete-ownership, start/stop, clone and interrupted-clone suites pass on each host; pre-existing containers, images and volumes identical before upgrade and after tests. Evidence: evidence/experimental4/. The confirmed duplicate APT source on the build host (podmesh.list, identical to xavdp.list) was backed up and removed.
- [x] Rehearse package removal, reinstallation, rollback and re-upgrade on one lab host: `apt-get remove` (not purge) removes binaries, service and enablement while retaining identity and journal; a universe started through PodMesh kept running throughout; reinstall replays verified operations as historical results; rollback to experimental3 from the host's APT cache (sha256-checked) runs on the experimental4 journal with its create/delete contract and honestly rejects start; re-upgrade restores ownership so the pre-removal universe is stopped and deleted through the API; unrelated resources unchanged. Evidence: evidence/experimental4/*/check-package-rehearsal.json. Not covered: purge, the other two hosts, clean-host installation, rollback of the experimental4-only journal fields beyond an added table.
- [ ] Test interrupted operations and clone-of-host identity rejection. Covered so far: interrupted clone commit (experimental3), and service kills during start observation and during a stop wait (experimental4, see Lifecycle). Clone-of-host identity rejection and other interruptions remain untested.
- [ ] Validate integrated ShaperOS deployment separately.

### 2. Lifecycle

- [x] Create a stopped, managed container from a local immutable image ID without network.
- [x] Verify retry deduplication, incompatible operation ID rejection, stopped-container deletion, running-container protection and preservation of unrelated containers on all three hosts. Evidence: evidence/lifecycle-*.json.
- [x] Add explicit start/stop through the API and CLI (0.1.0~experimental4), validated on all three hosts with Alpine fixtures: journal-recorded ownership (label-only, borrowed, replaced and unmanaged targets refused without Podman effect); start reports running only as observed, a short-lived application as not running with its exit code, and a runtime failure with the observed state; stop requires a declared `timeout_seconds` and `on_timeout` (`kill` reports forced escalation, `leave_running` never kills and fails honestly on timeout); retries of verified operations are labelled historical and carry a fresh observation that is checked against Podman; failed operations are re-evaluated; a service kill during start observation does not start the application twice; a service kill during a stop wait leaves Podman's `stopping` state with the application alive, reported as running and completed by retrying the same operation; a stale interrupted stop is refused against a newer run; running universes, identity, journal and ownership survive a service restart (conmon runs outside the service cgroup); an unrelated running container is untouched; every Podman event on API-managed universes falls inside a PodMesh API request window except post-exit cleanup and one natural application exit. Evidence: evidence/experimental4/*/check-start-stop.json. Not covered: networking, volumes, paused-state operations, restart policies, health checks, rootless Podman, host reboot, concurrent administrative Podman writes during an operation.
- [x] Tighten deletion (0.1.0~experimental4): delete requires a verified journal creation for the same universe and container ID; absent-target cleanup removes only snapshot images whose clone operation is recorded for that universe, source and (once verified) image and source container, and retains impostors. Refusal and retention tests pass on all three hosts. Evidence: evidence/experimental4/*/check-delete-ownership.json.
- [x] Validate network-disabled cloning of a stopped, mount-free source recorded in the same host's PodMesh journal, on all three hosts with 0.1.0~experimental3: new UUID and container identity, byte-identical copied data, independent writes in both directions, unchanged source (identity, state, labels, command, filesystem diff), retry replay, incompatible operation-ID reuse rejected, snapshot reuse after a simulated interrupted attempt, clone of a clone, clone survival after source deletion, snapshot image removal on delete, and unchanged pre-existing containers/images/volumes. Refused without Podman effect: same/invalid/missing source, label-only or replaced source, unmanaged target name, running or paused source, foreign snapshot reference. Evidence: evidence/experimental3/*/check-clone.json. Not covered: networked clones and unique IP allocation, volumes or bind mounts, running sources, rootless Podman, sources not created by PodMesh.
- [x] Interrupted clone: service killed with SIGKILL during a 384 MiB snapshot commit, restarted by systemd, same operation retried; one clone, one snapshot image, matching data hash, no Podman temporary leftovers, on all three hosts. Evidence: evidence/experimental3/*/check-clone-interrupt.json. The kill landed during the commit in every run; a real kill between commit and container creation was not reproduced (that retry path is covered only by the simulated test above).
- [ ] Integrate and validate migration through the service, including return journeys, interrupted transfer/restore and recovery. Existing standalone kit evidence does not validate the service API. Progress, not completion: an experimental source-side preflight, reservation and checkpoint API (default rootful store, musl processes, packaged podmesh-vzcriu runtime) was exercised in the development tree on one lab host only; not packaged, no transfer, destination, restore, exclusion or reservation release. See docs/MIGRATION-INTEGRATION.md. Self-reviewed with six corrections, then independently counter-reviewed (no blocking defect; its corrections applied) and rerun on the same host (docs/REVIEW-MIGRATION-SOURCE.md). Next: the destination side defined in docs/MIGRATION-PROTOCOL.md.
- [ ] Validate networking and persistent volumes; do not extrapolate network-disabled tests.

### 3. Availability and fractal operation

- [ ] Validate periodic coherent disk/memory recovery points and quantify rollback.
- [ ] Test host loss, partition and reconnection; distinguish unreachable from stopped.
- [ ] Prove exclusion before activating a replacement for an exclusive universe.
- [ ] Test manager recovery and DNS/bootstrap dependencies.
- [ ] Exercise governor/makers and child supervision across all three hosts.
- [ ] Record recovery time, data loss, manual interventions and operational conclusions.

### 4. Sequential storage experiment

- [ ] After current tests, cleanly stop the three lab VMs, add one 60 GB disk each and restart; verify exact disk identities and free host capacity.
- [ ] Test LVM2 conventional volumes and thin pools, snapshots, clone independence, restoration and bounded saturation behavior.
- [ ] Preserve evidence, remove only disposable LVM test storage, reuse the same disks for ZFS and repeat comparable tests including incremental send/receive.
- [ ] Preserve evidence, remove only disposable ZFS test storage, reuse disks for Btrfs and repeat; explicitly test nested subvolume coverage.
- [ ] Compare practicality, installation, capacity consumption, transfer cost, reliability and recovery. Select based on measurements; keep backends optional.

### 5. Backup Server and public delivery

- [ ] Build separately deployable PodMesh Backup Server: versioned configuration/images/volumes/optional memory, retention, encryption, integrity checks and verified restore.
- [ ] Prefer ShaperOS internally; validate standalone and container/VM/VPS deployment without mandatory ShaperOS or hypervisor dependency.
- [ ] Publish generic host preparation guide; laboratory description is a separate example.
- [ ] Publish product page/documentation with evidence-qualified reliability claims and human-agent attribution; no unproven HA or scaling promises.
- [ ] Version source, packaging, tests and limitations coherently; do not publish private lab addressing as public configuration.

## Work allocation and completion discipline

Before delegating, choose model and effort based on task difficulty, risk and cost: Luna for bounded routine tasks, Terra for ordinary implementation/testing, Sol for more involved changes, Astra for difficult architecture or recovery decisions. These are selection heuristics, not quality guarantees. To preserve the operator's GPT budget, Claude Code is preferred for bounded implementation and execution of tests, with the relevant full context. Codex is used at requested review checkpoints and for difficult architecture or recovery problems. A reviewer must be independent of the implementation being reviewed; Claude reviewing its own change is not an independent counter-view. Record the actual model and effort when known, and verify findings before applying corrections.

Continue authorized work without waiting for status prompts. Update this tracker as evidence is obtained. At each coherent delivery, apply governance, human/organizational and technical/adversarial review; record whether independent counter-review was actually available. Never mark the complete product proven based on an isolated test.

## Complete package chain

- [ ] Inventory every required runtime component and its upstream license/distribution constraints.
- [ ] Package project-specific services, migration helpers and patched CRIU/vzcriu as needed, with pinned source provenance and explicit conflicts/alternatives to avoid silently replacing system tools.
- [x] Publish `podmesh-vzcriu` and the separate controller/node helper packages through signed experimental APT; install and verify their paths/runtime selection on all three existing lab hosts. Thirty helper dispatch tests passed after independent Codex review of Claude Code changes. Sources: vzcriu fork commits `bcfb270` and `a753ead`. This is not clean-host qualification, package rollback proof, or a migration through the PodMesh API.
- [ ] Declare ordinary dependencies on supported Debian packages.
- [ ] Publish the required experimental packages through signed APT and prove clean installation, upgrade and rollback on all three lab hosts.
- [ ] Keep genesis and attribution visible, linking the public migration fork and reproduction kit without claiming generic HA from selected experiments.

## Alpine preference

- [ ] Prefer Alpine-based images for suitable services and universe components.
- [ ] Validate build/runtime dependency compatibility and full operation, including checkpoint/restore where relevant.
- [ ] Document justified exceptions; preserve standalone Debian host and APT delivery support.
