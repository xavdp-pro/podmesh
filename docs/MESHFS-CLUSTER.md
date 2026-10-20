# MeshFS fixed-membership cluster

Clustered mode is explicit. An existing standalone namespace is not automatically
converted or imported. Peer discovery through `join` is not data replication.

Use a separate empty data directory for each replica. Give every replica a JSON
configuration with the same cluster name and peer map, and a different stable
`node_id`. Peer values are routable HTTP or HTTPS URLs, including their ports.
For example, with addresses supplied by your deployment:

```json
{"cluster":"storage","node_id":1,"peers":{"1":"http://replica-a:7443","2":"http://replica-b:7443","3":"http://replica-c:7443"}}
```

Set `MESHFS_CLUSTER_TOKEN` to the same protected random secret (at least 32 bytes)
on all replicas. Protect the entire API with your deployment's network isolation
and authenticated TLS gateway: the token authenticates internal cluster RPCs,
not the existing public client API. Plain HTTP does not encrypt data or tokens.
Start all replicas with `meshfs daemon --cluster-config <file> --data-dir <dir>
--socket <owned-socket> --port <port>`. Only on initial creation, supply
`--bootstrap` to **one** replica. Omit it on every subsequent restart.

The membership is fixed to an odd number of at least three replicas. Do not
change peer identities, erase the Raft directory, re-bootstrap a lost cluster,
or substitute an empty disk for an existing identity. Disk replacement and
membership changes need a separate recovery procedure and are not implemented
by `join`. Restart with the same durable directory and configuration.

Mutations use OpenRaft and a SQLite WAL journal with synchronous durability.
An acknowledged write has committed through a majority and applied on the
leader. Other replicas catch up asynchronously. All client file and namespace
reads use a leader quorum barrier; a minority fails rather than returning a
locally stale file. This provides ordered writes; simultaneous writes to the
same path are serialized, and the last committed write becomes current. An
unacknowledged request has an uncertain outcome and must be reconciled by
reading the namespace before retrying. This is not an application merge policy.

A returning replica replays missing journal entries or receives a file-backed
snapshot containing namespace metadata and immutable chunks. Snapshot transfer
and chunk integrity checks use bounded buffers. Metadata is checksummed; each
chunk is verified against its content address. The new snapshot format is
versioned in the topology fingerprint: it does not support mixing old and new
replicas or an implicit upgrade of old cluster state.

For VM images and other large files, use `meshfs put` / `meshfs get`, or stream
raw bytes with `POST /api/v1/meshfs/files/stream?volume=default&path=disk.qcow2`
and `GET /api/v1/meshfs/files/download?volume=default&path=disk.qcow2`.
There is no 16 MiB file ceiling on this path. The compatibility Base64 JSON
upload remains bounded to 16 MiB; it is not the large-image interface. The CLI
streams files, validates the download's SHA-256 ETag and replaces an output file
only after the transfer has completed successfully.

The ingest path replicates immutable blocks of at most 4 MiB, then atomically
commits the complete file's metadata. Its full-file digest is calculated during
ingestion and checked during download; committed references are validated
without rescanning an entire disk while holding the Raft state-machine lock.
Interrupted ingest does not publish a truncated replacement. Already committed
immutable blocks can be reused after a quorum check. A failed final response
still has an uncertain outcome; reconcile the path before retrying.

File snapshots retain referenced chunks. Interrupted uploads also retain staged
blocks; no automatic garbage collection is implemented. Plan disk space for CAS,
journal, snapshot transfer and retained versions. Snapshot metadata has a 128 MiB
frame limit; data is transferred in bounded frames independently of file length.
Disk capacity and namespace metadata size remain real limits.

These APIs transfer complete offline images. They do **not** implement a FUSE
mount, NBD device or random-write block interface for a running VM. Validate a
restored image with its native tools and filesystem before use. Live VM storage,
safe membership replacement, disk replacement and production operations require
separate implementations and qualification. Existing services are not migrated
by enabling this mode.

`status` contains local diagnostic counters and replication metrics. A successful
quorum probe is point-in-time readiness, not a permanent availability guarantee.
The node list reports configured URLs and reachability; reachable does not mean
fully caught up. The configured voting set is never reduced on a timeout.

Keep standalone directories and installed services intact while qualifying a
candidate in separate directories and ports. A successful local CAS integrity
check does not establish replication, failover or catch-up correctness.
