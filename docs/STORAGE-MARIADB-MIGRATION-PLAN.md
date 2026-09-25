# Storage migration plan — SQLite to MariaDB

Status: operator-directed plan, 2026-09-25. Design intent, not field qualification.
Owner: Xavier de Poorter. Repository: public `podmesh` (code and product docs).

## 1. Why migrate

1. **Capacity and write concurrency** — Node journals and manager stores see growing
   operation, recovery-point and replication traffic. SQLite’s single-writer model
   and WAL growth become the bottleneck before host CPU is saturated.
2. **Backup harmonization** — SHAPER universes already prove restore with
   `mariadb-dump` per functional boundary (V1.14 Rule 26). PodMesh today copies
   `.sqlite` files and bespoke integrity checks; operators carry two recovery
   playbooks.
3. **Alignment with future Shaper cell** — The intended manager-inside-a-universe
   layout assumes function-owned MariaDB and the same four-way identity
   (`functional-slug` = system user = DB user = database). SQLite in the manager
   is recorded legacy in convergence inventory, not the long-term target.

This plan does **not** claim SHAPER Rule 26 compliance until each store is
qualified with isolated MariaDB, restore proof and documented boundaries.

## 2. Scope

### In scope

| Store | Current artefact | Owning component | Target |
| --- | --- | --- | --- |
| Node local state | `state.sqlite` under node state dir (`podmesh/src/*`, `lib.rs`) | `podmesh` / `podmeshd` | Dedicated MariaDB instance for the **node** functional role |
| Manager replica history | `manager.sqlite` (+ WAL/SHM) (`experiments/manager-ha`, manager packaging, V3-5 resident) | Manager universe / `podmesh-managerd` | Dedicated MariaDB instance for the **manager replica** role |
| Qualification & campaigns | Scripts referencing `sqlite_integrity_result`, file copy of `manager.sqlite*` | `packaging/podmesh-manager/qualification/*`, lab tools | Generalized **store integrity** contract + `mariadb-dump` / restore proof |

### Out of scope (this plan)

- **Workstation HA gate files** (`gate-m-u2.sqlite`, fencing lab gates) — retire with
  V3 workstation removal; do not migrate to MariaDB on the operator laptop.
- **Replacing MariaDB with one shared instance** for node + manager — forbidden by
  the isolation goal; two instances minimum on a host that runs both.
- **Field MED-M5 campaigns** until storage migration is lab-proven on the integrated
  branch and gates 1–3 allow installation of a new image.
- **Automatic in-place upgrade of med-pmox guests** still on image `aa79926`.

### Branch strategy

- **Authoritative editing**: public clone `podmesh`, branch from current integration
  line (`codex/v3-5-med-integrated` or successor), not from stale `main` alone.
- **Evidence**: `podmesh-lab/records/storage-mariadb-*/` for measurements and restore drills.

## 3. Design principles

1. **One MariaDB per PodMesh functional role per host** — Node store and manager
   store never share a database or credentials, even in one universe container set.
2. **Same contracts, different engine** — Operation IDs, epochs, receipts, import
   refusal rules and signing ledger semantics stay unchanged; only the durability
   layer changes.
3. **Explicit configuration** — Replace path-only `database_path` pointing at a file
   with a structured **store profile** (engine, DSN, database name, credential
   source). SQLite remains supported as **dev/lab fallback** until cutover, then
   deprecated behind a feature flag with a removal date.
4. **Crash-safe ordering preserved** — Every mutation that today relies on SQLite
   transactions + fsync must map to InnoDB transactions with documented isolation
   (default `REPEATABLE READ`; justify any downgrade).
5. **Backup is part of definition of done** — No phase closes without
   `mariadb-dump` → empty instance → restore → integrity + behavioral regression
   on that phase’s test slice.

## 4. Target architecture

```text
Host (or manager universe)
├── podmesh-node role
│   ├── process: podmeshd
│   └── MariaDB instance A (database: podmesh-node or slug-aligned name)
│       └── schemas: lifecycle, migration, publisher, recovery_points, secrets, …
└── podmesh-manager replica role (when installed)
    ├── process: podmesh-managerd (resident)
    └── MariaDB instance B
        └── schemas: manager-ha history, receipts, audit, signing ledger tables, …
```

**Credential layout (Shaper-aligned when integrated):**

- Password file e.g. `/apps/<functional-slug>/etc/mysql/localhost/passwd` (mode 0600),
  or container-local equivalent mounted from host secret dir for manager replicas.
- Application connects only as `<functional-slug>` to matching database; admin
  proof uses local root CLI path for qualification scripts.

**Packaging options (pick one per role in Phase 0 decision record):**

| Option | Pros | Cons |
| --- | --- | --- |
| **Sidecar `brick-mariadb` Podman** per role | Matches SHAPER universe pattern, same backup story | More containers, port/volume wiring |
| **Embedded MariaDB in manager universe image** (node: host-managed or nested) | Fewer moving parts on small lab | Must not merge node+manager DB |
| **Host systemd MariaDB instances** (lab only) | Fast spike | Weaker portability story for adopters |

Recommendation: **sidecar or co-located Podman MariaDB per role** for product
story; host systemd only for early spikes.

## 5. Phased delivery

### Phase 0 — Decision record (1–2 days)

Deliverables in `podmesh-lab/records/storage-mariadb-2026-09-25/` (or dated successor):

- Chosen packaging option per role.
- Functional slugs and naming (`podmesh-node`, `podmesh-manager` vs universe UUID).
- Performance hypothesis to falsify (writes/sec, recovery-point retention job,
  manager import burst).
- Explicit **non-goals** (no field votes, no gate closure).

Update `docs/EXPERIMENTAL-SCOPE.md` with a short pointer to this plan.

### Phase 1 — Storage abstraction (engineering)

1. Introduce internal crate or module `podmesh-storage` with trait e.g.
   `DurableStore`: transactions, prepared statements, integrity check, busy/lock
   timeout mapping.
2. Implement **SQLite backend** (wrap existing `rusqlite` paths) — no behavior change.
3. Implement **MariaDB backend** (`mysql` or `sqlx` with `mysql`, sync API acceptable
   for first cut if resident is sync today).
4. Feature flag / config: `store.engine = sqlite | mariadb`.
5. **Tests**: same suite runs twice in CI matrix (GitHub or local) where feasible.

Exit: all existing unit/integration tests green on SQLite; MariaDB backend compiles
and passes a **minimal** schema bootstrap test.

### Phase 2 — Node (`state.sqlite`) migration

1. Move DDL from scattered `CREATE TABLE` in `src/*.rs` to **versioned migrations**
   (one directory, sequential SQL files, engine-specific sections only where
   required).
2. Map SQLite types → MariaDB (`INTEGER`, `TEXT`, `BLOB`, `DATETIME(6)`, JSON
   where used).
3. Replace hard-coded `state.sqlite` in `lib.rs` with configured store.
4. Add offline tool `podmesh-storage-migrate`:
   - read-only open SQLite source;
   - bulk copy or logical export into empty MariaDB target;
   - row-count and checksum spot checks per table;
   - refuse if target non-empty unless `--force` with backup mandate.
5. **Backup script** for node role: `mariadb-dump` scoped to node database; document
   in `docs/BACKUP-AND-RESTORE.md` (new or extended).
6. Re-run node-heavy tests: recovery points, publisher, migration collector,
   secrets, lifecycle.

Exit: lab record with dump size, restore time, and full `tests/` delta green on
MariaDB node store.

### Phase 3 — Manager (`manager.sqlite` / `manager-ha`) migration

1. Port `experiments/manager-ha` durable layer to `podmesh-storage` (largest risk:
   immutable history, receipt binding, periodic full verification).
2. Replace `database_path` file semantics in config JSON with store profile; keep
   temporary alias: if `database_path` ends with `.sqlite`, treat as legacy SQLite
   profile for one release.
3. Rename qualification fields in a **backward-compatible** way:
   - `sqlite_integrity_result` → `store_integrity_result` (accept both in readers
     for one release).
4. Update `capture-host.sh`, `compare-evidence.py`, campaign scripts to dump/check
   MariaDB instead of copying `.sqlite*` (WAL/SHM copy path removed for MariaDB).
5. Re-run manager test suites: resident tests, activation qualification, vote tool
   offline validation, integrated 16-check pair on P3 with `umask 077`.

Exit: 96+ manager / 82+ node tests green on MariaDB; lab record with restore after
fake “store corruption” drill.

### Phase 4 — Performance and retention

1. Benchmarks on `podmesh-build` LXC (or equivalent): sustained write load on
   recovery-point retention + manager import replay.
2. Compare SQLite vs MariaDB at 50th/95th latency for configured operations;
   document when SQLite remains acceptable (dev only).
3. Tune pool size, `innodb_buffer_pool_size` guidance for lab hosts (documentation
   only, not hard-coded).

Exit: published numbers in lab record; decision to drop SQLite default for
packaged `.deb`.

### Phase 5 — Packaging, docs, deprecation

1. Debian packages declare dependency on MariaDB client tools for backup hooks.
2. OCI/manager universe images: start MariaDB sidecar or bundled instance; entrypoint
   waits for DB readiness before `podmesh-managerd`.
3. **Deprecation**: SQLite engine flagged `legacy`; removal target version noted in
   CHANGELOG and `EXPERIMENTAL-SCOPE.md`.
4. Update convergence note (SHAPER side): PodMesh manager/node stores → MariaDB
   when qualification record exists (separate PR in SHAPER repo if needed).

Exit: frozen scope file for one Debian increment (INTENT ladder step 3); not step 6
until field campaign.

### Phase 6 — Field (gated)

Prerequisites:

- Gates 1–3 on storage-migration image, not `aa79926`.
- Written mandate for med-pmox lab guests only.
- Restore drill includes MariaDB dump restore, not file copy.

Steps: new image → non-voting manager set first → compare evidence → only then
MED-M5 voting campaign per existing gate 4 plan.

## 6. SQL and semantic risks

| Risk | Mitigation |
| --- | --- |
| SQLite `AUTOINCREMENT` vs MariaDB `AUTO_INCREMENT` | Central migrations; test max-id continuity after migrate tool |
| Partial indexes / `IF NOT EXISTS` differences | Avoid SQLite-only DDL; test both engines in CI |
| `sqlite_master` introspection in tests | Replace with information_schema queries abstracted in storage layer |
| Manager “full store verification” timing | Re-benchmark; MariaDB may need longer read snapshot or replica lag awareness |
| Signing ledger fsync ordering | Keep “write ledger before signature leaves” invariant in MariaDB transaction boundaries |
| Cross-engine restore of mixed host | Migration tool run once at cutover; no dual-write except brief read-only SQLite + write MariaDB forbidden |

## 7. Testing matrix

| Layer | SQLite (regression) | MariaDB (required for phase exit) |
| --- | --- | --- |
| Rust unit tests in `src/` | Until deprecation | Phase 2+ |
| Python lab tests | Selected | Phase 2+ node, Phase 3 manager |
| `manager-resident` / `manager-ha` | Until Phase 3 cutover | Phase 3 |
| Qualification activation | Legacy path one release | Primary path Phase 3 |
| 16-check integrated pair | Optional parallel | Mandatory before Phase 6 |

Add one **migration-specific** test: SQLite fixture → migrate tool → MariaDB → run
integrity + one end-to-end operation path.

## 8. Rollback

- During Phase 2–3: config flag back to SQLite with **empty** MariaDB or restored
  from pre-cutover dump; never run dual-write.
- After field install: rollback = restore MariaDB dump + prior package version;
  document in campaign plan alongside existing gate 3 hypervisor rules.

## 9. Open questions for operator

1. Sidecar MariaDB vs embedded in manager universe image for med-pmox?
2. Slug names for Rule 26 alignment when manager lives inside Shaper OS later?
3. Accept temporary **two** backup formats in one release (file copy for old
   guests, dump for new) — yes/no and for how long?
4. Priority: node first (less gate coupling) or manager first (V3-5 qualification
   surface)? **Recommended: node Phase 2, then manager Phase 3.**

## 10. Success criteria (summary)

- Node and manager each use an **isolated** MariaDB with documented credentials.
- Standard backup = logical dump + proven restore on lab topology.
- Integrated test suites pass on MariaDB at least at current SQLite coverage.
- SQLite remains available one release cycle for dev rollback, then deprecated.
- No MED-M5 or vote-key activity until Phase 6 prerequisites are met.

---

Related: [INTENT.md](../INTENT.md) (delivery ladder), [EXPERIMENTAL-SCOPE.md](EXPERIMENTAL-SCOPE.md),
private `podmesh-lab/status/CURRENT-STATE.md` (gates). SHAPER Rule 26 and
[September MariaDB profile](https://github.com/xavdp-pro/SHAPER-OS-V1.15/blob/main/docs/profiles/SEPTEMBER-CONTAINER-MARIADB.md)
state the ecosystem target this migration aligns with, without auto-qualifying PodMesh.
