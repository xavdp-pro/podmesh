# PodMesh acceptance test sequence

Status: planned. A successful build is not runtime validation.

Codex implements and executes the initial acceptance tests. Claude Code subsequently performs an independent counter-review with the requirements, source code and recorded evidence. High-availability testing starts only after the baseline lifecycle gates pass.

All destructive tests use explicitly identified disposable lab resources. Preserve unrelated workloads. Record versions, operation IDs, source and destination identities, expected results, actual results and failures.

| Gate | Scope | Required evidence |
| --- | --- | --- |
| 1. Installation | Install the package and service on each of the three hosts; restart, upgrade and uninstall | Service responds through the CLI/API; identities persist across restart and upgrade; uninstall follows its documented state-retention policy |
| 2. Local foundations | Identity, inventory, journal and error handling | Distinct host identities; inventory agrees with independent Podman inspection; failures are reported honestly; journal survives restart |
| 3. Creation | Create a disposable universe from a declared image and configuration | Correct image digest, identity, configuration and running workload; retries do not create extra instances |
| 4. Cloning | Clone a disposable universe | New universe UUID and unique IP when networking is enabled; independent writable data; source remains unchanged; retries do not duplicate clones |
| 5. Deletion | Delete explicitly selected disposable resources | Only the selected resource is removed; volume-retention behavior is explicit; repeated deletion is handled consistently |
| 6. Migration | Move a disposable universe between hosts, including return journeys | Same universe identity and in-memory marker; progressing workload; source no longer active; destination controllable; archive integrity verified; inject transfer/restore failures and verify recovery |
| 7. Network continuity | Exercise supported direct-network and optional WireGuard modes | Unique addresses, correct routing after movement, reachability and separately measured connection continuity; unsupported cases remain explicit |
| 8. High availability | After baseline gates pass: host loss, network partition, reconnect and manager recovery | No unauthorized duplicate activation; preserved local operation where allowed; measured recovery time and data rollback; reconciled histories; explicit handling when safe takeover cannot be established |

Cloning creates a new universe. Migration preserves the original universe identity. A standby replica shares the logical universe identity but must not independently activate merely because the source becomes unreachable.

Current qualified status (0.1.0~experimental3, standalone Linux mode, three lab hosts): gates 1–5 are partially exercised for network-disabled, volume-free containers only — installation and APT upgrade, local foundations, creation from a local image ID, stopped-source cloning and stopped-container deletion. Uninstall, running workloads, networking (including unique clone IPs), volumes, migration integration and HA are not validated through PodMesh. See DELIVERY-CHECKLIST.md for the exact scope and evidence. Earlier standalone migration-kit experiments do not satisfy these service acceptance gates.

The same contracts must ultimately be checked in standalone Linux mode and ShaperOS-integrated mode. Passing one mode does not validate the other.
