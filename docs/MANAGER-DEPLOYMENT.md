# Experimental resident manager package and deployment boundary

Status: the reviewed resident-manager candidate from commit `5319ab1` was built,
published through signed experimental APT and installed with its service disabled
and inactive on three existing laboratory hosts. The package-only comparisons
passed. No configured resident replication, DNS service or HA claim exists yet.

## Purpose and isolation

`podmesh-manager` is the resident process candidate for one replica of the logical
control-services universe on one Linux host. It is intended to retain and exchange
control facts such as replica identity, immutable history, peer progress, conflicts
and later DNS publication evidence. It is not the PodMesh lifecycle daemon, an
alternative Governor, a Maker, an automatic activation authority or a shared
filesystem replacement.

The design replaces the operational dependency on one shared live cluster
filesystem with independently durable replicas that exchange immutable records.
It does **not** make a network partition safe by itself. Exclusive activation still
needs a qualified external effect gate and exclusion proof. The detailed identity,
partition and reconciliation model is in [CONTROL-SERVICES-UNIVERSE.md](CONTROL-SERVICES-UNIVERSE.md);
the current executable laboratory boundaries are in
[MANAGER-HA-ACCEPTANCE.md](MANAGER-HA-ACCEPTANCE.md).

The first executable milestone exercised the manager as standalone local
processes. This isolated its persistence, replication, refusal and recovery
behavior from an orchestration environment. The package is now installed but the
resident service has deliberately not been configured or activated. The preferred integrated target then
runs each host-bound replica inside a ShaperOS universe so it can reuse ShaperOS
logging, observation and parent-supervision contracts. It remains the same single
logical manager across those replicas. Standalone operation on a compatible Linux
host remains required for users who do not deploy ShaperOS; neither mode is evidence
for the other and both require their own qualification.

This package deliberately coexists with the existing experimental components:

| Component | Package/service | This package's relationship |
| --- | --- | --- |
| Local lifecycle API | `podmesh` / `podmesh.service` | Untouched: no file overlap, dependency, restart, stop or socket reuse |
| Observation API | `podmesh-web-observer` / `podmesh-web-observer.service` | Untouched: no file overlap, dependency, restart, stop or socket reuse |
| Manager replica candidate | `podmesh-manager` / `podmesh-manager.service` | Separate binary, user, configuration, state and runtime directory |

No package maintainer script edits `/run/podmesh`, `/var/lib/podmesh`,
`/run/podmesh-web-observer`, `/var/lib/podmesh-web-observer`, their units or their
binaries. The manager package does not depend on Podman, CRIU or WireGuard.

## Package layout

The source-ready layout is in `packaging/podmesh-manager/`.

| Resource | Location | Ownership/mode |
| --- | --- | --- |
| Manager binary | `/usr/lib/podmesh-manager/podmesh-managerd` | root:root, `0755` |
| Service unit | `/usr/lib/systemd/system/podmesh-manager.service` | root:root, `0644` |
| Configuration directory | `/etc/podmesh-manager` | root:podmesh-manager, `0750` |
| Operator configuration and current lab pair keys | `/etc/podmesh-manager/config.json` | root:podmesh-manager, `0640` |
| Persistent state | `/var/lib/podmesh-manager` | podmesh-manager:podmesh-manager, `0750` |
| Ephemeral runtime files | `/run/podmesh-manager` | podmesh-manager:podmesh-manager, `0700` |
| Example configuration | `/usr/share/podmesh-manager/config.example.json` | root:root, `0644` |

`podmesh-manager` is a dedicated system user with a non-login shell. Installation
creates directories and reloads systemd, but does not generate a logical manager
identity, configuration, certificate, WireGuard interface, firewall rule, network
listener or DNS record. It also does not enable, start or restart the service.
The unit has `ConditionPathExists=/etc/podmesh-manager/config.json`,
`IPAddressDeny=any`, an `AF_UNIX` address-family allowlist and the default
environment explicitly sets network mode to `disabled`.

The current `manager-resident` laboratory schema keeps pair HMAC keys inline as
`network.peers[].shared_key_hex`; it has no `key_file` field or separate key
directory. The package therefore does not create or promise a keys directory. Its
operator-owned configuration file is the private key boundary and must remain
`0640` or stricter. A future migration to separate key files requires a reviewed
schema change, key lifecycle and a package update before this layout changes.

The service runs unprivileged and uses systemd state/runtime directories, a private
temporary directory, a strict read-only host filesystem except for declared
state/runtime paths, no ambient capabilities and a constrained Unix-only address
family. `Restart=no` avoids an automatic recovery claim before actual host
qualification. Its hardening must be qualified with the exact package candidate on
disposable hosts, then requalified when its storage, transport or DNS behavior
changes; hardening that prevents a required, reviewed operation is not silently
relaxed.

## Build contract

Packaging only assembles an explicitly supplied manager binary. It never compiles
Rust, downloads inputs, generates keys, installs a package, starts a service,
publishes to APT or contacts a host. The current source candidate provides the
documented `--config`, `--state-dir` and `--runtime-dir` command-line contract,
performs pure offline configuration validation, refuses networking unless the
reviewed mode is explicitly selected, and reports a build/version identifier.

The supplied binary must be an amd64 ELF named exactly `podmesh-managerd`; its
SHA-256 is verified before and after staging. The packager derives the highest
required `GLIBC_x.y` symbol and refuses a value above the declared `libc6 (>=
2.39)` baseline. A binary with no dynamic GLIBC symbol is reported as `none`; the
conservative Debian dependency remains and that result is not proof of static-build
portability. With a trusted source checkout:

```sh
SOURCE_DATE_EPOCH="$(git show -s --format=%ct HEAD)" \
  packaging/podmesh-manager/build-deb.sh \
  /trusted/output/podmesh-managerd \
  "$(sha256sum /trusted/output/podmesh-managerd | cut -d ' ' -f 1)" \
  '0.1.0~manager1' /tmp/podmesh-manager-packages
```

Identical binary, version, source timestamp and packaging tools should produce a
byte-identical `.deb`. That establishes deterministic assembly, not a reproducible
Rust compilation, functional manager, secure key lifecycle or HA.

Run the assembly-only gate before review or publication:

```sh
packaging/podmesh-manager/test-packaging.sh
```

## Network, firewall and WireGuard

The package opens no port and makes no firewall or routing changes. The current
candidate refuses runtime startup before opening its database, lock or sockets while
the `PODMESH_MANAGER_NETWORK_MODE=disabled` environment contract is in force. It
requires an explicit `authenticated-static-peers` opt-in and a separately reviewed
systemd network drop-in before it can run as a resident service. Transport configuration,
known peers and current inline pair keys are supplied only through the protected
operator-owned configuration file. Received data must never enroll a peer, choose a
filesystem path, change a configuration path or widen a replica's scope.

Existing routed host connectivity is the baseline. WireGuard is optional: when
used, it may encrypt manager exchange and artifact transfers, but it is neither a
package dependency nor an authority mechanism. A future transport qualification
must record selected endpoints without placing private addresses, peer keys or
tokens in public documentation, packages or evidence.

## Explicit installation and activation procedure

This procedure is the laboratory qualification checklist for the reviewed local
candidate. Complete it independently for each of the three declared lab hosts.
The current `podmesh-manager-resident-lab` source now implements the package CLI
contract, strict declared state/runtime boundaries, offline validation and an
explicit `authenticated-static-peers` runtime opt-in. Claude Code Opus independently
reviewed that source boundary. Candidate `0.1.0~manager1+g5319ab150fc3` was rebuilt
from reviewed committed source, verified against signed repository metadata,
published to the experimental suite and installed with the service disabled and
inactive on three existing laboratory hosts. The package-only evidence is recorded
in [REVIEW-MANAGER-PACKAGE-QUALIFICATION.md](REVIEW-MANAGER-PACKAGE-QUALIFICATION.md).
The example configuration is its exact current JSON schema. Later configuration,
activation and recovery stages remain open.

1. Record a pre-install inventory outside the candidate package: host identity,
   package versions, `podmesh.service` and `podmesh-web-observer.service` PIDs,
   their unit state, both Unix socket metadata, and full rootful Podman inventory.
   Hash the evidence and use stable host aliases instead of public/private addresses.
2. Verify the signed APT metadata, package version, package SHA-256, binary
   SHA-256 and file list. Confirm the reviewed candidate contract names the intended
   source commit; this contract assertion is not an independent source-to-binary
   reproducibility proof. Confirm package paths have no overlap with either existing
   PodMesh package.
3. Install the package only. Confirm that no manager service starts or enables,
   no manager configuration or inline key material exists unless the operator created it, and both
   existing services retain their pre-install PIDs and inventories.
4. Create a unique per-host replica configuration from the example, preserving one
   declared logical manager identity but using distinct host and replica IDs. The
   current laboratory key fields stay inside the protected configuration; never copy
   them into a public artifact. Create an otherwise empty temporary runtime directory
   owned by `podmesh-manager`, then run offline validation as that account so both
   declared directories have the same ownership as the process:

   ```sh
   install -d -o podmesh-manager -g podmesh-manager -m 0700 /run/podmesh-manager
   runuser -u podmesh-manager -- /usr/lib/podmesh-manager/podmesh-managerd \
     --config /etc/podmesh-manager/config.json \
     --state-dir /var/lib/podmesh-manager \
     --runtime-dir /run/podmesh-manager \
     --validate-config
   rmdir /run/podmesh-manager
   ```

   The final command must remove only the empty directory created for validation;
   any unexpected content is a refusal that requires investigation.
5. Exercise the package's default-disabled gate before adding any override. Record
   the result of `systemctl start podmesh-manager.service`, but do not use that
   command's exit status as the verdict: a `Type=simple` start job can return before
   the process refusal is observed. Wait for the process outcome and require
   `systemctl show` to report `ActiveState=failed`, `Result=exit-code`,
   and `ExecMainStatus=1`; preserve the matching journal evidence, then run
   `systemctl reset-failed podmesh-manager.service`. A skipped condition or an
   inactive unit is not this proof. The attempt must leave no manager database,
   lock, control socket, TCP/UDP listener, firewall or routing change. This is a
   negative installation proof, not a running-service qualification.
6. To qualify resident exchange, install an explicit, separately reviewed systemd
   drop-in that selects `authenticated-static-peers`, removes `IPAddressDeny=any`
   and widens the address-family set only as required by the accepted transport.
   Start **only** `podmesh-manager.service`, then verify its UID, directory modes,
   state ownership, exact arguments, declared listeners and logs.
7. Run only the acceptance scenarios whose prerequisite gates in
   `MANAGER-HA-ACCEPTANCE.md` are satisfied. Initial resident-service qualification
   must prove the manager's own identity/state lifecycle before replica exchange,
   effects, DNS, migration or workload activation.
8. Record a post-test independent inventory. The lifecycle and observer PIDs,
   socket modes, package versions, rootful Podman inventory and unrelated
   workloads must match the pre-install record. Explain every intended manager
   artifact separately.

## Upgrade, removal, purge and rollback

An upgrade installs a new binary and reloads systemd but deliberately does not
restart the manager. The unit never retries or restarts automatically. The operator
snapshots the manager state and preserves a verified previous package before an
explicit restart. A restart after upgrade is a new qualification step, not evidence
from the old process.

`apt remove podmesh-manager` stops and disables only `podmesh-manager.service`.
`apt purge podmesh-manager` follows the same policy: identity, state,
configuration (including current inline pair key material) and the system account
are deliberately retained.
This makes recovery explicit and prevents a package command from erasing the
evidence that explains an incident. Reinstallation must re-open the same local
identity only when the operator has verified that it is the intended replica.

Rollback is forbidden until the resident manager provides and qualifies a schema
compatibility contract. A downgrade can remove checks that a newer state relies on. Before any
rollback trial, capture the package/state/configuration hashes, verify a supported
prior binary and schema path on a disposable copy, and prove that replayed
operation IDs remain deterministic. Never replace a current manager database with
an older copy while it could make an exclusive decision.

## Three-host qualification evidence

For each stage, preserve one evidence bundle per host and a cross-host comparison:

| Stage | Required external proof |
| --- | --- |
| Pre-install | Existing service PIDs, unit states, sockets, package versions and Podman inventory |
| Install | Package signatures/hashes/file list; no manager process; existing PIDs/inventory unchanged |
| Configure | Different replica ID per host; same logical manager ID; config/key ownership and mode; no secrets retained in public evidence |
| Default-disabled refusal | Failed unit with exit status 1; no manager database, lock, socket or listener; matching journal evidence |
| Resident activation | Separately reviewed network opt-in; only manager unit changed; PID/UID/arguments; state/runtime modes; only declared listeners |
| Restart | Same replica identity and state checksum; existing service PIDs/inventory unchanged |
| Upgrade | Previous and new package/binary hashes; explicit restart; schema/version compatibility result |
| Remove/purge/reinstall | Manager-only stop/disable; retained identity/state; lifecycle/observer PIDs and inventories unchanged |
| Rollback | Disposable copied state only until schema/replay contract is qualified |

The independent observer, host package database, systemd manager and rootful
Podman CLI provide evidence from outside the resident manager. The manager's own
health endpoint or log cannot prove its effect, its exclusion guarantees, host
death or unchanged workloads.

## Open requirements after package-only publication

- Establish key generation, rotation, revocation and operator recovery rules.
- Qualify the implemented authenticated static-peer transport on three disposable
  hosts, including confidentiality and endpoint policy.
- Bind replica history to a durable external effect gate before any exclusive
  activation, route or DNS publication.
- Repeat clean full-host installation when suitable disposable clean hosts are
  available; the current three-host result covers existing laboratory hosts.
- Qualify explicit activation, restart, upgrade, removal, purge and supported
  rollback separately before claiming those lifecycle phases.
- Publish only the package scope supported by the recorded evidence through the
  signed experimental APT suite.

The package boundary was designed before the compatible binary so its separate
state, privilege, network and lifecycle contract could be reviewed without
accidentally modifying the currently qualified PodMesh daemon or observer. The
current candidate preserves that boundary.

Claude Code Opus independently counter-reviewed this source-ready boundary. Its
findings on directory modes, Debian lifecycle ordering, network isolation,
configuration truthfulness, licensing, GLIBC portability and maintainer-script
regression checks were corrected. The final closure review reported no remaining
blocker or important finding for this undeployed scope. Claude Code Opus then
reviewed the compatible CLI, path boundary, offline validation and network refusal;
after two port-race corrections it reported no remaining blocker or important
finding for the local-only increment. Signed publication and disabled-service
installation now pass on three existing hosts. Configuration, activation, service
recovery and HA qualification remain open.
