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

## Uncertain write outcomes

A transport timeout or EIO does not establish cancellation of server work. The
request ID protects retries of the same operation; it is not an expired-writer
session fence. Before recovering a writer after an uncertain outcome, reconcile
its data and pending operations. The current adapter does not provide automatic
fencing between an old delayed session and a new mount. Qualification of that
recovery boundary is required before production VM storage use.
