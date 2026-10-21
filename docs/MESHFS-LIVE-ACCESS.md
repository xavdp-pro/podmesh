# Experimental live access

`meshfs mount --peer URL --peer URL --peer URL --volume NAME MOUNTPOINT`
exposes one clustered flat-file volume with FUSE on Linux. Explicit peers are
required. The API must be on a trusted network; this adapter does not add public
API authentication. The mounting account must have FUSE/allow_other privileges.
Use a dedicated volume and one writer per file. This is an experimental access
adapter, not production VM storage or a complete POSIX filesystem.

Writes stage immutable 64 KiB blocks through the quorum log, then atomically
compare and replace the inode. Acknowledgement follows durable replication.
A stable semantic request ID makes transport retries idempotent within the retained replay generations. Reads obtain a
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

Each live mutation carries an immutable `epoch` obtained from
`GET /api/v1/meshfs/files/replay-epoch` **before** reading its source precondition,
and an explicit immutable source precondition in `base`:
`{"version":"<inode-version>"}`, or `{"version":null}` for an absent file.
The inode version combines a durable, cluster-wide monotonic revision with its
digest and timestamp. Rename also requires `destination` with the captured
version (or explicit absence) of its target. Both comparisons happen atomically
when applying the mutation. Delete/recreate consumes new revisions, preventing
reuse of a pathname's old version.

The FUSE client captures these versions once before its first send and retains
the same body and semantic request ID through transport retries. It never
refreshes a failed write's precondition to force it through. Missing an epoch or required
preconditions returns HTTP 428; an obsolete version returns HTTP 409. The FUSE
adapter reports these conflicts as `ESTALE`. Legacy inline and streaming upload
paths reject replacement of an existing live inode with HTTP 409; they cannot
bypass the versioned live interface.

Within a retained epoch, a byte-identical retry of an already committed semantic
request returns its original success without applying it again, even after a
newer mutation. Once its epoch expires, the retry returns HTTP 409 instead of
reapplying it; reconcile uncertain outcomes. A
never-committed delayed request with an obsolete version is rejected, including
after a new client mount. A zero-length write currently returns without storing
a deduplication record; a subsequent retry can report conflict after intervening
changes. It cannot overwrite bytes.

A transport timeout or EIO still does not establish cancellation of server work.
If its precondition remains current, a delayed request may succeed after the
caller timed out. Reconcile uncertain outcomes before recovering a writer.
Version comparison protects newer acknowledged mutations; it does not provide
exclusive writer leases, session expiry or distributed locking.

## Bounded historical metadata

Every 256 mutable command admissions, a durable monotonic replay epoch advances.
The current and immediately previous epoch remain admissible; this grace permits
an operation staged across one rollover to complete. Older or future epochs are
rejected before either deduplication lookup. Epochs, admission counters and entry
generations are part of the replicated state and snapshots. Request epochs and
preconditions are never refreshed during transport retries. The generic Raft
command envelope is fenced too. Immutable CAS staging uses its own cleanup
counter and does not advance mutable replay epochs or occupy result caches:
preparing a large inode cannot expire its own mutation. Its idempotence follows
from the verified block content address. A delayed create cannot reacquire authority just
because its pathname was deleted and its old dedup record was purged.

Each command-result and live-operation cache retains at most 512 entries.
Staged-block metadata retains all current and named-snapshot references, plus a
recent window of 262144 distinct staged block IDs; pruning occurs on rollover,
so up to 1280 additional historical block IDs can occur between prunes
(the inline legacy path can stage four blocks per admission, alongside up to
255 independent staging admissions). This is a bound
on historical overhead, not a fixed bound on useful live data or snapshots.
Staging more unique unreferenced blocks than that window before publishing an
inode can expire its staging references and makes commit fail explicitly. For
canonical 4 MiB chunks the window represents 1 TiB of distinct pending data;
for 64 KiB live blocks, the window covers the existing 16 GiB live manifest
limit. Concurrent uploads share it. Physical CAS blocks are not deleted by this change:
CAS disk-space reclamation is separate work.

The topology fingerprint uses `meshfs-live-v3-epochs`. All peers and live
clients must use the corresponding protocol. Mixed-version operation and an
in-place upgrade of an older cluster are not qualified. This change does not
establish production VM storage readiness or a complete backup/restore contract.


## Durable apply journal and replication batches

Applied Raft batches are persisted in a separate SQLite journal with their applied
marker in one FULL transaction before acknowledgement. Recovery reconstructs the
full machine from its checkpoint and ordered journal tail. Snapshot checkpoints
consolidate only covered batches; snapshot installation atomically replaces the
base and clears the previous tail. Legacy full-machine checkpoints remain valid
recovery bases. Do not interpret the checkpoint alone as current applied state.
The delta journal stores the applied command entries, including staged payload
bytes, until a checkpoint consolidates them. Size disk reserves for this journal
as well as CAS and retained Raft logs; small-object tests do not qualify
full-size payload storage or throughput.
Downgrading an active journal-backed store to a binary that ignores that journal
is unsupported; retain a consistent backup rather than replacing the binary.

Log reads select the requested index range in SQL. Replication splits multi-entry
batches above a 10 MiB serialized-entry budget using OpenRaft's size hint, without
changing the requested log-reader range. The request envelope adds a small amount.
An existing valid indivisible command may travel alone above that budget. Its
existing size limits and the 250 ms AppendEntries timeout still apply; this is
not a guarantee of full-size payload latency. The mutation deadline remains three
seconds, and a timeout still requires reconciliation before any retry.


Raft log readers and storage operations run in awaited blocking jobs admitted
through one shared FIFO lane across connection clones. SQLite mutex waiting and
FULL commits therefore do not occupy Tokio workers. The admission permit and
append completion callback stay with the blocking job through durable commit,
even if its awaiting future is dropped. Snapshot state cloning also runs off
Tokio. This preserves the persisted format; it does not guarantee that storage
contention or snapshot installation cannot exceed an operation deadline.
