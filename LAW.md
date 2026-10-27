# Law — podmesh (v2.3.0)

The following architectural and operational invariants govern the sovereign `podmesh` orchestrator (`v2.3.0`) across its core Rust node engine, manager control plane, `podmesh-ctl` CLI, and SHAPER OS integration boundaries. Every code change, packaging recipe, and deployment profile must preserve these laws without exception.

---

## 1. Rootless Podman Execution & Scoped Capability Boundary
- Functional units and managed workloads execute inside isolated **Podman** boundaries (rootless by default for application and functional units; no Docker daemon).
- Inherited image `VOLUME` declarations are suppressed on private unit launches; only explicitly declared and owned application and database volumes are mounted.
- Where host-level lifecycle or checkpoint/migration actions require elevated capabilities, they are mediated exclusively through the scoped **Host Adapter** (`src/host_adapter.rs`, `docs/HOST-ADAPTER-CONTRACT.md`). Delegated host capability never grants SHAPER Governor or Maker authority and never merges with the application user or database identity.

## 2. Unix Domain Socket Isolation (`/var/run/shaper`)
- Inter-unit and host-adapter control paths communicate over permission-scoped **Unix Domain Sockets** under `/var/run/shaper` (and unit-isolated runtime directories such as `/run/podmesh/api.sock`).
- Socket files enforce strict POSIX ownership and mode boundaries (`0600` or `0660` restricted to the declared unit group), preventing cross-unit ambient access or unauthorized local command injection.

## 3. Single-Writer Monotonic Epoch Fencing
- At most one active writer/manager or active universe placement may mutate a governed resource at any given epoch.
- Every failover, placement promotion, or takeover requires an **Ed25519-signed takeover certificate** naming the authority quorum policy digest (`sha256`) and a strictly increasing monotonic epoch (`epoch_new > epoch_current`).
- Stale epochs, expired leases, or unverified signatures fail closed immediately before any state mutation or container lifecycle transition occurs.

## 4. MariaDB Durable State (`SEP22-CONTAINER-MARIADB`)
- Durable application and orchestration state for declared functional units (`podmesh-node`, `podmesh-manager`) follows the SHAPER **`SEP22-CONTAINER-MARIADB`** profile (Rules 4 and 26).
- Each durable unit owns a **private MariaDB server instance** inside its isolated boundary (`slug = system user = DB user = database`), speaking over a local Unix socket or loopback interface without external shared-database coupling.
- High-volume observation tables are partitioned by workload and retention need (e.g., monthly partition administration with sealed-source handoff and verified restore).

## 5. Rule 38 Bounded Retries ($k \le 2$) & Idempotent Operations
- Every mutating request carries an explicit operation ID, target UUID, authorization reference, and preconditions.
- Reusing an operation ID must never duplicate an effect; incompatible reuse fails closed. Interrupted work must be reconciled against observed host/store state before retry.
- Automated recovery and retry loops are strictly bounded by **Rule 38 ($k \le 2$ retries)**. When $k \le 2$ recovery attempts fail to converge, the component halts further automatic retries, transitions deterministically to `DEGRADED`, preserves forensic evidence, and escalates to the supervising authority.

## 6. GEM-001 Zero Public Open Ports
- Under **GEM-001**, `podmesh` exposes **zero unauthenticated public open ports** on host network interfaces.
- Node and manager daemons bind exclusively to local Unix Domain Sockets (`/var/run/shaper`, `/run/podmesh`) or host-local loopback (`127.0.0.1`), with cross-host and operator access traversing authenticated SSH tunnels, WireGuard VPN meshes, or scoped Cloudflare/TLS publishers with mandatory session and Ed25519/token verification.
