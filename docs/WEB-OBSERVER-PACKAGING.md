# Observation-only explorer package

## Scope and isolation

`podmesh-web-observer` packages the explorer-enabled `podmeshd` as a separate,
experimental Debian amd64 service. It does not replace the `podmesh` package or
its service and does not package the browser gateway. No network listener is
created: the API is a root-only Unix socket.

| Resource | Observer location |
| --- | --- |
| Binary | `/usr/lib/podmesh-web-observer/podmeshd` |
| Unit | `podmesh-web-observer.service` |
| Socket | `/run/podmesh-web-observer/api.sock` |
| Persistent identity and observation journal | `/var/lib/podmesh-web-observer` |

The unit sets `PODMESH_READ_ONLY=1`. The corresponding Rust guard accepts only
`identity`, `capabilities`, `inventory`, `observations`, `container_details`, and
`host_resource_metrics`.
This is an application guard, not a privilege sandbox. The service runs as root
because the current explorer observes the default rootful Podman store and uses
`podman exec` to inspect running nested stores. Queries can update observation
journals and launch short-lived observation processes inside a running container;
"observation-only" does not mean zero filesystem or process effects. It must not
start, stop, clone, delete or migrate workloads. Do not expose this socket to
untrusted clients. The separate identity is an observer identity, not an automatic
replacement for the lifecycle daemon's host UUID.

## Build contract

Supply a trusted binary built from the intended explorer source, its explicit
SHA-256, the package version and a source timestamp. Packaging performs no Cargo
build, download, host installation, service activation or APT publication.

```sh
PODMESH_PACKAGE_VERSION=0.1.0~observer2 cargo build --release --locked --bin podmeshd
SOURCE_DATE_EPOCH="$(git show -s --format=%ct HEAD)" \
  packaging/web-observer/build-deb.sh \
  target/release/podmeshd \
  "$(sha256sum target/release/podmeshd | cut -d ' ' -f 1)" \
  '0.1.0~observer2' /tmp/podmesh-observer-packages
```

The script rejects checksum mismatches and non-amd64 ELF inputs, normalizes file
ownership and timestamps, and uses deterministic xz package compression. Identical
inputs and toolchain versions should produce byte-identical packages. This does
not establish that independently compiled Rust binaries are reproducible. The
hash records the supplied artifact, not proof of the read-only guard. Validate the
binary's reported version and guard before deployment. Build against the supported
Debian runtime: declared libc/libgcc dependencies are conservative baseline names,
not automatic ABI analysis. Inspect required symbols and qualify on each target.
No CRIU dependency is needed by the enabled observations.

## Explicit installation and activation

```sh
sudo apt install /path/to/podmesh-web-observer_0.1.0~observer2_amd64.deb
sudo systemctl enable --now podmesh-web-observer.service
sudo systemctl status podmesh-web-observer.service
```

Installation only reloads systemd. An upgrade deliberately leaves the running
process in place; explicitly restart after qualification to load the new binary.
Point the trusted gateway's observer connection to the observer socket, retaining
the lifecycle socket for authorized actions. Never silently substitute the
observer for an action endpoint.

Removal stops and disables only this unit. Persistent state is preserved even on
purge for explicit recovery. The runtime directory is service-owned; startup
removes only its dedicated stale socket. No maintainer script edits or deletes
`/run/podmesh`, `/var/lib/podmesh`, or the installed lifecycle binary.

## Target qualification performed on 2026-09-12

- [x] Record the lifecycle service PID and inventory before installation.
- [x] Verify the package file list and supplied binary checksum.
- [x] Activate the observer explicitly; verify socket mode `0600`, directory mode
  `0700`, separate identity/state, and no TCP listener.
- [x] Query a running nested container and a stopped container; verify stopped workloads stay stopped.
- [x] Request a lifecycle mutation and verify the observation-only rejection and
  unchanged workload inventory from outside the observer.
- [x] Restart and confirm persistent observer identity and journal.
- [x] Remove/purge and reinstall on one host; confirm state preservation.
- [x] Recheck the lifecycle service PID and original workloads remain unchanged.
- [x] Qualify a later package-version upgrade; reinstalling the same version is not upgrade proof.

The original checked items passed on three Debian 13 laboratory hosts for package
`0.1.0~observer1`; purge/reinstall was exercised on the third host. Package
`0.1.0~observer2` was then built from commit `909fc5b`, published to the signed
`trixie-experimental` APT suite and installed as an actual package upgrade on all
three hosts. Its binary SHA-256 is
`f43fcc419a85d43f71b3903b3fb08b0b25ce1297266762f50377b6e5c21b0ac5`;
the amd64 package SHA-256 is
`7de1314de2eabb041ca52cee26f7504d9ae2ebf3b63c482e77c026d2c7a16c68`.
Before publication, the repository container was snapshot-backed up. Each
observer service was then explicitly restarted to load the upgraded binary. The
persistent observer state remained present, the lifecycle daemon PID
and complete rootful Podman inventory matched their recorded pre-upgrade values, the
dedicated socket remained mode `0600`, mutations were refused, and RAM, CPU/load
and graph-root filesystem capacity returned as independently sourced observations.
The local gateway then displayed live metrics from all three dedicated observer
sockets without enabling actions.

These checks do not establish clean-host compatibility, long-duration operation
or HA. Container details cover the default rootful nested store only. Rootless
stores, fractal authority, HA, per-universe persistent-volume capacity, and
production deployment are outside this package's claim.
