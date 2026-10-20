# Experimental live access

`meshfs mount --peer URL --peer URL --peer URL --volume NAME MOUNTPOINT`
exposes one clustered flat-file volume with FUSE on Linux. Explicit peers are
required. The API must be on a trusted network; this adapter does not add public
API authentication. The mounting account must have FUSE/allow_other privileges.
Use a dedicated volume and one writer per file. This is an experimental access
adapter, not production VM storage or a complete POSIX filesystem.

Writes stage immutable 64 KiB blocks through the quorum log, then atomically
compare and replace the inode. Acknowledgement follows durable replication.
A stable semantic request ID makes transport retries idempotent. Reads obtain a
quorum-checked inode and verify each immutable block. The client tries all
configured peers on transport/quorum failures within a bounded 20-second
failover deadline, retaining the semantic request ID even when a response body
is interrupted. Kernel data caching is disabled;
flush/fsync requires a successful quorum barrier. Loss of majority fails I/O.

Live manifests use a `merkle-sha256:` digest over file size and ordered block
references; this is not a whole-file SHA. Whole-file downloads for these inodes
are explicitly rejected: read via the mount or bounded range endpoint. Existing
4 MiB streamed files retain their whole-file SHA contract. Import a disk by
writing its bytes through the mount into a new live file, before attaching it.
The experimental manifest limit is 262144 blocks (16 GiB), independent of the
streaming API's large-file support.

The initial adapter supports flat files, random reads/writes, truncate, create,
rename and unlink. Permissions/ownership/timestamps are fixed presentation
metadata, not persisted POSIX metadata. Directories, xattrs, hard links,
distributed file locks, multi-writer fencing and HA promotion are not provided.
Do not attach the same VM disk to multiple writers. Test QEMU workloads require
an explicit single-writer configuration; these tests do not qualify the
Proxmox storage plugin or VM orchestration. PodMesh manages Podman applications;
QEMU/Proxmox remains responsible for VMs.

## Versioned mutations and uncertain outcomes

Each live mutation carries an explicit immutable source precondition in `base`:
`{"version":"<inode-version>"}`, or `{"version":null}` for an absent file.
The inode version combines a durable, cluster-wide monotonic revision with its
digest and timestamp. Rename also requires `destination` with the captured
version (or explicit absence) of its target. Both comparisons happen atomically
when applying the mutation. Delete/recreate consumes new revisions, preventing
reuse of a pathname's old version.

The FUSE client captures these versions once before its first send and retains
the same body and semantic request ID through transport retries. It never
refreshes a failed write's precondition to force it through. Missing required
preconditions return HTTP 428; an obsolete version returns HTTP 409. The FUSE
adapter reports these conflicts as `ESTALE`. Legacy inline and streaming upload
paths reject replacement of an existing live inode with HTTP 409; they cannot
bypass the versioned live interface.

A byte-identical retry of an already committed semantic request returns its
original success without applying it again, even after a newer mutation. A
never-committed delayed request with an obsolete version is rejected, including
after a new client mount. A zero-length write currently returns without storing
a deduplication record; a subsequent retry can report conflict after intervening
changes. It cannot overwrite bytes.

A transport timeout or EIO still does not establish cancellation of server work.
If its precondition remains current, a delayed request may succeed after the
caller timed out. Reconcile uncertain outcomes before recovering a writer.
Version comparison protects newer acknowledged mutations; it does not provide
exclusive writer leases, session expiry or distributed locking.

The topology fingerprint uses `meshfs-live-v2-preconditions`. All peers and live
clients must use the corresponding protocol. Mixed-version operation and an
in-place upgrade of an older cluster are not qualified. This change does not
establish production VM storage readiness or a complete backup/restore contract.
