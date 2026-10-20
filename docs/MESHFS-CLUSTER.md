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

A returning replica replays missing journal entries or receives a snapshot
containing namespace metadata and referenced immutable chunks. File snapshots
also retain their referenced chunks. No automatic chunk garbage collection is
performed. Snapshots and journal payloads are buffered in memory; clustered
uploads are currently limited to 16 MiB per file and internal HTTP bodies to
128 MiB. These bounds are not a design for large VM disk storage. Large namespace
snapshots, streaming, safe membership replacement, client retry semantics and
production operational qualification remain work to complete before deploying
this mode for VM images.

`status` contains local diagnostic counters and replication metrics. A successful
quorum probe is point-in-time readiness, not a permanent availability guarantee.
The node list reports configured URLs and reachability; reachable does not mean
fully caught up. The configured voting set is never reduced on a timeout.

Keep standalone directories and installed services intact while qualifying a
candidate in separate directories and ports. A successful local CAS integrity
check does not establish replication, failover or catch-up correctness.
