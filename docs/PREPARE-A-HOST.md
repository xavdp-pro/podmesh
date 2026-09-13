# Prepare a host

## Current validated target

Debian 13 on amd64, with systemd and rootful Podman. Physical servers, virtual machines and VPS are possible host types; the provider must allow the required container facilities. The current package is experimental. ShaperOS is optional; integrated execution is not yet validated.

## Install from the signed repository

Install curl and CA certificates using your distribution packages, then:

```sh
sudo install -d -m 0755 /etc/apt/keyrings
curl -fsSL https://deb.xavdp.pro/keys/xavdp-archive-keyring.gpg -o /tmp/xavdp.gpg
gpg --show-keys --with-fingerprint /tmp/xavdp.gpg
```

Check the signing fingerprint through a trusted channel before installing the key. The repository's current documented fingerprint is `870B13865A81810E19109668A7DE52F62814551B`.

```sh
sudo install -m 0644 /tmp/xavdp.gpg /etc/apt/keyrings/xavdp.gpg
printf '%s\n' 'deb [signed-by=/etc/apt/keyrings/xavdp.gpg] https://deb.xavdp.pro/podmesh trixie-experimental main' | sudo tee /etc/apt/sources.list.d/xavdp.list
sudo apt-get update
sudo apt-get install podmesh
sudo systemctl is-active podmesh
sudo podmesh capabilities
sudo podmesh identity
sudo podmesh inventory
```

APT verifies signed repository metadata and package hashes. Do not use `trusted=yes` or bypass signature checks. The daemon starts during package installation. Its Unix socket is root-only and is not a public network API.

## Persistence and access

The default database lives in `/var/lib/podmesh`; the local API is `/run/podmesh/api.sock`. The service uses the host's default rootful Podman store. Do not assume a separately configured Podman store is included in its inventory. Keep backups of persistent state. Package removal currently retains state, even on purge, for explicit recovery.

The service records a machine identity to reject accidental state reuse on another host. Restoring or cloning a whole host needs an explicit identity adoption procedure; that procedure is not yet a supported command.

`authorization_ref` is audit provenance, not a remotely verified credential. Access to the root-only socket is the current trust boundary. Do not expose it through an unauthenticated network proxy.

## Optional storage and transport

LVM2, LVM thin, ZFS, Btrfs and WireGuard are planned optional capabilities. They are not requirements for the initial installation. Do not reformat an existing disk to install this package. The patched migration runtime and its helper shim install from the same repository, and from `0.1.0~experimental5` the service package itself carries the experimental migration operations. They are qualified for one workload shape — Alpine, musl, network-disabled, mount-free — between two hosts with identical kernel, runtime and image identity. A reservation is not fencing, and direct administration bypasses it: do not enable this pathway for ordinary workloads.

### Storage recommendation for PodMesh Backup Server

PodMesh Backup Server can run on an ordinary Linux filesystem, but dedicated,
snapshot-capable storage is strongly recommended. The candidates to qualify are Btrfs,
ZFS and LVM2 thin. Btrfs is the provisional general-host default, ZFS is the provisional
dedicated-backup-datastore choice, and LVM2 thin is the compatibility choice. The lab
comparison, rather than this preference, decides which recommendations become
supported defaults.

With a qualified snapshot backend, PodMesh can quiesce a protected universe, create a
local immutable capture, release it, and perform the slower transfer from the snapshot.
On an ordinary root filesystem, the generic portable path keeps the protected universe
stopped while its archive is built. Installation preflight must display which capture
class is available and must never reformat or repurpose an existing disk automatically.

For containers, Alpine is preferred where tested. That preference does not change the Debian host package target.
