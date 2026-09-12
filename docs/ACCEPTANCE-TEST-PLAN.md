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

Current qualified status (standalone Linux mode). Gates 1–5 are partially exercised for network-disabled, volume-free containers only: installation and APT upgrade, package removal/reinstallation/rollback/re-upgrade on one host, local foundations, creation from a local image ID, explicit start and stop of simple Alpine workloads with observed outcomes, stopped-source cloning, and deletion restricted to journal-recorded containers. `0.1.0~experimental4` carried that scope on all three lab hosts.

`0.1.0~experimental6` is now published through signed APT and installed on all three laboratory hosts. Each passed eight installed-service suites, 143 checks, with identities and unrelated inventories preserved. The target source suite was repeated after temporarily importing the missing Debian fixture, closing two conditional refusal checks; the image was then removed. Package removal/reinstallation/downgrade to retained experimental5/re-upgrade passed on one host with a continuously running witness. Clean Debian container dependency installation passed, but does not qualify a clean full host.

The final byte-identical daemon also passed an 81-check two-host collector suite, including memory continuity, pidfd reclaim and actual daemon death after a committed class-2 effect followed by exactly-once recovery. Transfer and destination recovery qualification remains restricted to the documented Alpine musl, network-disabled, volume-free workload. See REVIEW-COLLECTOR-COMPLETION.md for scope and hashes.

Networking (including unique clone IPs), volumes, purge, clean-host installation, gate 7 and gate 8 are not validated through PodMesh. A reservation is not fencing. See DELIVERY-CHECKLIST.md for the exact scope and evidence. Earlier standalone migration-kit experiments do not satisfy these service acceptance gates.

The same contracts must ultimately be checked in standalone Linux mode and ShaperOS-integrated mode. Passing one mode does not validate the other.
