mod activation;
mod boot_restore;
mod cleanup;
mod manager;
mod network;
mod publisher;
mod recovery_point;
mod retention;
mod schema;
mod secrets;
mod signing;
// The journal's engine, named once (docs/STORE-CONFIGURATION.md). Public so that the tools and
// the manager tree may open a store; `storage` below is Podman's graph, not this.
mod health;
mod storage;
pub mod store;
pub use manager::control_relay;
pub use network::reconcile as reconcile_network;
pub use publisher::withdraw_at_startup as withdraw_unentitled_publishers_at_startup;
mod collector;
mod lifecycle;
mod migration;
mod recovery;
mod restore;
mod transfer;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use store::{migrations, DurableStore, Engine, SqliteStore, StoreConfig, Value as Stored};

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Read-only outer-container shape rules for `migration_preflight` / `migration_checkpoint` assess.
/// Integration tests and lab fixtures use this without opening Podman.
pub fn migration_shape_blockers_for_profile(
    container: &Value,
    migration_profile: &str,
) -> Result<Vec<String>, String> {
    let profile = migration::MigrationProfile::parse(migration_profile)
        .map_err(|e| e.to_string())?;
    Ok(migration::migration_shape_blockers(container, profile))
}

/// The file a node's store profile is read from, under its state directory, when it is there.
/// A node with no such file keeps the journal it has: SQLite, in `state.sqlite` beside it.
pub const STORE_PROFILE_FILE: &str = "store.json";
/// Another profile file, named by the operator. Named and unreadable is a refusal, not a default.
pub const STORE_PROFILE_ENVIRONMENT: &str = "PODMESH_STORE_PROFILE";

/// The store profile this state directory runs under (`docs/STORE-CONFIGURATION.md`).
///
/// The default is what every node has today, and a node that names no profile is a node whose
/// behaviour is unchanged. A profile that cannot be read is never replaced by the default: an
/// operator who wrote one is asking for it.
pub fn store_profile(dir: &Path) -> Result<StoreConfig, Box<dyn std::error::Error>> {
    let path = match std::env::var_os(STORE_PROFILE_ENVIRONMENT) {
        Some(named) => PathBuf::from(named),
        None => {
            let beside = dir.join(STORE_PROFILE_FILE);
            if !beside.exists() {
                return Ok(StoreConfig::for_state_dir(dir));
            }
            beside
        }
    };
    let text = fs::read_to_string(&path).map_err(|e| {
        format!(
            "The store profile {} could not be read: {e}",
            path.display()
        )
    })?;
    let document: Value = serde_json::from_str(&text)
        .map_err(|e| format!("The store profile {} is not JSON: {e}", path.display()))?;
    Ok(StoreConfig::from_json(&document, Some(dir))?)
}

/// A manager replica's durable store, open, as whichever engine its profile named.
pub enum ManagerStore {
    Sqlite(Connection),
    Durable(Box<dyn DurableStore>),
}

/// How `manager-ha` opens the manager journal: SQLite keeps a `rusqlite` connection; MariaDB
/// keeps the durable store contract (`experiments/manager-ha` journal profile entry path).
pub enum ManagerJournal {
    Sqlite(Connection),
    Durable(Box<dyn DurableStore>),
}

impl ManagerStore {
    pub fn engine(&self) -> Engine {
        match self {
            ManagerStore::Sqlite(_) => Engine::Sqlite,
            ManagerStore::Durable(store) => store.engine(),
        }
    }

    /// Hand the journal to `manager-ha` or resident wiring without assuming a file-backed engine.
    pub fn into_journal(self) -> ManagerJournal {
        match self {
            ManagerStore::Sqlite(db) => ManagerJournal::Sqlite(db),
            ManagerStore::Durable(store) => ManagerJournal::Durable(store),
        }
    }

    /// Legacy SQLite file path for code that still takes `rusqlite::Connection` directly.
    ///
    /// MariaDB profiles must use [`ManagerStore::into_journal`] and the `manager-ha` journal
    /// profile opener (`ConfiguredStore::open` with the `mariadb` feature), not this method.
    pub fn into_connection(self) -> Result<Connection, Box<dyn std::error::Error>> {
        match self {
            ManagerStore::Sqlite(db) => Ok(db),
            ManagerStore::Durable(store) => Err(format!(
                "store.engine is {engine}: the manager journal opened and its schema is at version {version}, \
                 but this API still returns only a SQLite connection. \
                 Use ManagerStore::into_journal and experiments/manager-ha with store profile engine mariadb \
                 (Phase 3 of docs/STORAGE-MARIADB-MIGRATION-PLAN.md). \
                 Keep database_path on a .sqlite file for the legacy file path.",
                engine = store.engine(),
                version = migrations::manager_version(),
            )
            .into()),
        }
    }
}

/// Resolve the manager replica store profile beside a state directory.
///
/// When `store.json` is present (or [`STORE_PROFILE_ENVIRONMENT`] names a file), that profile is
/// read. Otherwise the journal is SQLite at `sqlite_database_path`, which is what resident
/// configuration names today as `network.database_path`.
pub fn resolve_manager_store_profile(
    profile_dir: &Path,
    sqlite_database_path: &Path,
) -> Result<StoreConfig, Box<dyn std::error::Error>> {
    let named = profile_dir.join(STORE_PROFILE_FILE);
    if std::env::var_os(STORE_PROFILE_ENVIRONMENT).is_some() || named.exists() {
        return store_profile(profile_dir);
    }
    Ok(StoreConfig::for_manager_sqlite_path(sqlite_database_path))
}

/// Open the manager replica journal under the profile it is configured with.
pub fn open_manager_store(config: &StoreConfig) -> Result<ManagerStore, Box<dyn std::error::Error>> {
    config.validate()?;
    match config.engine {
        Engine::Sqlite => {
            let mut store = SqliteStore::open(&config.sqlite)?;
            migrations::apply_manager(&mut store)?;
            migrations::refuse_incomplete_manager_cutover(&mut store)?;
            Ok(ManagerStore::Sqlite(store.into_connection()))
        }
        Engine::Mariadb => {
            let mut store = store::open(config)?;
            migrations::apply_manager(store.as_mut())?;
            migrations::refuse_incomplete_manager_cutover(store.as_mut())?;
            Ok(ManagerStore::Durable(store))
        }
    }
}

/// A node's journal, open, as whichever engine its profile named.
pub enum NodeStore {
    /// The engine every node runs today: the `rusqlite` connection every module still takes.
    Sqlite(Connection),
    /// A journal reached through the store contract, which is MariaDB today.
    Durable(Box<dyn DurableStore>),
}

impl NodeStore {
    pub fn engine(&self) -> Engine {
        match self {
            NodeStore::Sqlite(_) => Engine::Sqlite,
            NodeStore::Durable(store) => store.engine(),
        }
    }

    pub fn connection(&self) -> Option<&Connection> {
        match self {
            NodeStore::Sqlite(db) => Some(db),
            NodeStore::Durable(_) => None,
        }
    }

    /// Serve one local API request without changing the SQLite operation path.
    ///
    /// SQLite keeps the existing [`handle`] byte for byte. A durable engine serves the reads,
    /// lifecycle, and secret operations ported below; every other known operation is refused by
    /// name until the module that owns it speaks [`DurableStore`].
    pub fn handle(&mut self, request: &Value) -> Value {
        match self {
            NodeStore::Sqlite(db) => handle(db, request),
            NodeStore::Durable(store) => handle_durable(store.as_mut(), request),
        }
    }

    /// Refuse a durable lifecycle candidate that needs an unported startup reconciler.
    /// This check reads only the journal, before the daemon opens its API socket.
    pub fn validate_startup_scope(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if let NodeStore::Durable(store) = self {
            validate_durable_startup_scope(store.as_mut())?;
        }
        Ok(())
    }

    /// The connection the node's operations take, or a refusal naming why there is none.
    ///
    /// This is where Phase 2 stops. The node's schema is versioned and installs on both engines,
    /// and the profile decides which one is opened -- but `handle` and every module below it
    /// still speak `rusqlite`, so a journal that is not a file has nothing to hand them. A node
    /// configured for MariaDB is refused here, by name, rather than served from a file it was not
    /// configured to use.
    pub fn into_connection(self) -> Result<Connection, Box<dyn std::error::Error>> {
        match self {
            NodeStore::Sqlite(db) => Ok(db),
            NodeStore::Durable(store) => Err(format!(
                "store.engine is {engine}: the journal opened and its schema is at version {version}, \
                 but this build's operations still read and write the node's journal as SQLite \
                 (Phase 2 of docs/STORAGE-MARIADB-MIGRATION-PLAN.md ports them). \
                 Set store.engine to sqlite to run this node.",
                engine = store.engine(),
                version = migrations::node_version(),
            )
            .into()),
        }
    }
}

/// Conservative candidate precondition until publisher/network startup reconciliation is ported.
/// Historical rows also refuse: their absence from the host cannot be inferred from a journal.
fn validate_durable_startup_scope(
    store: &mut dyn DurableStore,
) -> Result<(), Box<dyn std::error::Error>> {
    for table in [
        "network_declaration",
        "network_peer_pools",
        "network_allocations",
        "network_routes",
        "network_effects",
        "publishers",
        "publisher_events",
        "publisher_transitions",
        "publisher_takeover_verified",
    ] {
        if store
            .query_one(&format!("SELECT 1 FROM {table} LIMIT 1"), &[])?
            .is_some()
        {
            return Err(format!("store_engine_unsupported: durable startup refused because {table} contains state requiring SQLite-only publisher/network reconciliation; preserve this journal and reconcile with a compatible implementation before cutover").into());
        }
    }
    for row in store.query("SELECT request FROM operations", &[])? {
        let request: Value = serde_json::from_str(row.text(0)?).map_err(|_| {
            "durable startup refused: cannot establish publisher/network scope from an invalid journal request"
        })?;
        let operation = request["operation"]
            .as_str()
            .ok_or("durable startup refused: journal request has no operation identity")?;
        if operation.starts_with("network_")
            || operation.starts_with("publisher_")
            || request["network_profile"].as_str() == Some(network::PROFILE_MANAGED)
        {
            return Err("store_engine_unsupported: durable startup refused because operation history includes publisher/network effects not supported by this candidate; preserve and reconcile the original state before cutover".into());
        }
    }
    Ok(())
}

/// Open the node's journal under the profile it is configured with, and bring its schema up.
///
/// Whichever engine carries it, the journal is the same set of tables at the same version: the
/// migrations in `src/store/migrations/node/` are applied in order, and the store records which
/// of them it has. An incomplete MariaDB profile is refused before anything is opened.
pub fn open_node_store(
    dir: &Path,
    config: &StoreConfig,
) -> Result<NodeStore, Box<dyn std::error::Error>> {
    fs::create_dir_all(dir)?;
    config.validate()?;
    let opened = match config.engine {
        Engine::Sqlite => {
            let mut store = SqliteStore::open(&config.sqlite)?;
            migrations::apply(&mut store)?;
            migrations::refuse_incomplete_cutover(&mut store)?;
            bind_to_this_host(&mut store)?;
            NodeStore::Sqlite(store.into_connection())
        }
        Engine::Mariadb => {
            let mut store = store::open(config)?;
            migrations::apply(store.as_mut())?;
            migrations::refuse_incomplete_cutover(store.as_mut())?;
            bind_to_this_host(store.as_mut())?;
            NodeStore::Durable(store)
        }
    };
    lifecycle::prepare_scratch(&dir.join("podman-tmp"))?;
    manager::prepare_host_state(dir);
    migration::prepare(&dir.join("migrations"))?;
    transfer::prepare(dir)?;
    Ok(opened)
}

/// Bind the journal to the machine it belongs to, and give the host its UUID if it has none.
///
/// Written through the store contract rather than in SQLite's dialect: `INSERT OR IGNORE` is one
/// engine's, so the row is read and then written inside a transaction, which is the same fact on
/// both. One process opens a node's journal, so there is no second writer to race here.
fn bind_to_this_host(store: &mut dyn DurableStore) -> Result<(), Box<dyn std::error::Error>> {
    let machine = fs::read_to_string("/etc/machine-id")?.trim().to_string();
    let uuid = fs::read_to_string("/proc/sys/kernel/random/uuid")?
        .trim()
        .to_string();
    let held = store.query_one(
        "SELECT value FROM metadata WHERE `key` = ?",
        &[Stored::from("machine_id")],
    )?;
    if let Some(previous) = &held {
        if previous.text(0)? != machine {
            return Err(
                "State belongs to a different host; explicit identity adoption required".into(),
            );
        }
    }
    let mut tx = store.transaction()?;
    if held.is_none() {
        tx.execute(
            "INSERT INTO metadata(`key`, value) VALUES(?, ?)",
            &[Stored::from("machine_id"), Stored::from(machine)],
        )?;
    }
    if tx
        .query_one(
            "SELECT value FROM metadata WHERE `key` = ?",
            &[Stored::from("host_uuid")],
        )?
        .is_none()
    {
        tx.execute(
            "INSERT INTO metadata(`key`, value) VALUES(?, ?)",
            &[Stored::from("host_uuid"), Stored::from(uuid)],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// The node's journal as every module still takes it, under the profile the state directory names.
pub fn open_state(dir: &Path) -> Result<Connection, Box<dyn std::error::Error>> {
    open_node_store(dir, &store_profile(dir)?)?.into_connection()
}
fn inventory() -> Result<Value, Box<dyn std::error::Error>> {
    // Fixed command; no caller-controlled shell or command arguments.
    let mut child = Command::new("/usr/bin/podman")
        .args(["ps", "--all", "--format", "json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    // Drain both pipes while waiting, bounding retained output without blocking the child.
    fn drain(mut input: impl std::io::Read) -> Vec<u8> {
        let mut out = Vec::new();
        let mut b = [0u8; 8192];
        loop {
            match input.read(&mut b) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if out.len() + n <= 4 * 1024 * 1024 {
                        out.extend_from_slice(&b[..n]);
                    }
                }
            }
        }
        out
    }
    let a = thread::spawn(move || drain(stdout));
    let b = thread::spawn(move || drain(stderr));
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if Instant::now() > deadline {
            child.kill()?;
            child.wait()?;
            return Err("Podman inventory timeout".into());
        }
        thread::sleep(Duration::from_millis(50));
    };
    let output = a.join().map_err(|_| "Output reader failed")?;
    let errors = b.join().map_err(|_| "Error reader failed")?;
    if !status.success() {
        return Err(format!("Podman failed: {}", String::from_utf8_lossy(&errors)).into());
    }
    Ok(serde_json::from_slice(&output)?)
}

fn sqlite_only_operation(operation: &str) -> bool {
    matches!(
        operation,
        "migration_preflight"
            | "migration_checkpoint"
            | "migration_authorize_transfer"
            | "migration_complete_transfer"
            | "migration_retire_source"
            | "migration_release"
            | "migration_abandon"
            | "migration_restore_local"
            | "migration_destination_preflight"
            | "migration_restore"
            | "migration_restore_abort"
            | "garbage_collect_plan"
            | "garbage_collect_apply"
            | "collection_retention_declare"
            | "collection_hold_declare"
            | "collection_hold_release"
            | "collection_status"
            | "manager_status"
            | "manager_decision"
            | "manager_observe"
            | "manager_vote_ledger_init"
            | "manager_vote_ledger_mark_unadmitted"
            | "manager_vote_ledger_readmit"
            | "manager_decision_propose"
            | "publisher_declare"
            | "publisher_start"
            | "publisher_stop"
            | "publisher_status"
            | "publisher_observed"
            | "network_declare"
            | "network_undeclare"
            | "network_route_publish"
            | "network_route_withdraw"
            | "network_route_resume"
            | "network_reapply"
            | "network_status"
            | "activation_require"
            | "activation_acquire"
            | "activation_renew"
            | "activation_release"
            | "activation_supersede"
            | "activation_status"
            | "activation_fence"
            | "activation_fence_preview"
            | "recovery_point_prepare"
            | "recovery_point_status"
            | "recovery_point_restore"
            | "recovery_point_promote"
            | "recovery_point_stage"
            | "recovery_point_discard"
            | "recovery_point_resume"
            | "boot_restore"
            | "boot_restore_status"
            | "migration_status"
            | "volume_declare"
            | "volume_grow"
    )
}

fn durable_lifecycle_operation(operation: &str) -> bool {
    matches!(
        operation,
        "create" | "delete" | "clone" | "start" | "stop" | "pause" | "resume" | "resources"
    )
}

fn durable_capabilities(engine: Engine) -> Value {
    json!({
        "schemas": schema::all(),
        "schema_version": "podmesh-operation-schema/1",
        "version": option_env!("PODMESH_PACKAGE_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
        "operations": [
            "capabilities",
            "identity",
            "inventory",
            "observations",
            "storage_status",
            "host_status",
            "universe_stats",
            "create",
            "delete",
            "clone",
            "start",
            "stop",
            "pause",
            "resume",
            "resources",
            "secret_declare",
            "secret_remove",
            "secret_status",
        ],
        "store_engine": engine.as_str(),
        "unsupported_error_code": "store_engine_unsupported",
        "scope": "read-only local API, lifecycle create/delete/clone/start/stop/pause/resume/resources, and secret_declare/secret_remove/secret_status; other module-owned mutations remain SQLite-only",
    })
}

/// Local API carried by a non-SQLite journal.
///
/// Reads, lifecycle, and secret declare/remove/status go through [`DurableStore`]. Every other
/// module-owned operation stays on the named refusal until that module is ported.
fn handle_durable(store: &mut dyn DurableStore, request: &Value) -> Value {
    let operation = request
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or("");
    if durable_lifecycle_operation(operation) {
        let result = lifecycle::execute_store(store, request);
        let response = match result {
            Ok(data) => json!({"ok":true,"observed_at":now(),"data":data}),
            Err(error) => {
                let mut failure = json!({"ok":false,"observed_at":now(),"error":error.to_string()});
                if let Some(details) = error.downcast_ref::<lifecycle::Failure>() {
                    failure["details"] = details.details.clone();
                }
                failure
            }
        };
        if let Err(error) = store.execute(
            "INSERT INTO observations(observed_at, operation, result) VALUES(?, ?, ?)",
            &[
                Stored::from(now() as i64),
                Stored::from(operation),
                Stored::from(response.to_string()),
            ],
        ) {
            return json!({"ok":false,"error":format!("Observation persistence failed: {error}")});
        }
        return response;
    }
    if matches!(operation, "secret_declare" | "secret_remove" | "secret_status") {
        let result = secrets::execute_store(store, request);
        let response = match result {
            Ok(data) => json!({"ok":true,"observed_at":now(),"data":data}),
            Err(error) => json!({"ok":false,"observed_at":now(),"error":error.to_string()}),
        };
        if let Err(error) = store.execute(
            "INSERT INTO observations(observed_at, operation, result) VALUES(?, ?, ?)",
            &[
                Stored::from(now() as i64),
                Stored::from(operation),
                Stored::from(response.to_string()),
            ],
        ) {
            return json!({"ok":false,"error":format!("Observation persistence failed: {error}")});
        }
        return response;
    }
    if sqlite_only_operation(operation) {
        return json!({
            "ok": false,
            "observed_at": now(),
            "error_code": "store_engine_unsupported",
            "error": format!(
                "Operation {operation} is unavailable with store.engine={}: its module is still SQLite-only",
                store.engine()
            ),
            "operation": operation,
            "store_engine": store.engine().as_str(),
        });
    }
    let result: Result<Value, Box<dyn std::error::Error>> = (|| {
        Ok(match operation {
            "capabilities" => durable_capabilities(store.engine()),
            "identity" => {
                let row = store
                    .query_one(
                        "SELECT value FROM metadata WHERE `key` = ?",
                        &[Stored::from("host_uuid")],
                    )?
                    .ok_or("The journal carries no host_uuid")?;
                json!({"host_uuid": row.text(0)?})
            }
            "inventory" => json!({"containers":inventory()?,"store":"default rootful Podman"}),
            "observations" => {
                let rows = store.query(
                    "SELECT id, observed_at, operation FROM observations ORDER BY id DESC LIMIT 20",
                    &[],
                )?;
                let observations = rows
                    .iter()
                    .map(|row| {
                        Ok(json!({
                            "id": row.integer(0)?,
                            "observed_at": row.integer(1)?,
                            "operation": row.text(2)?,
                        }))
                    })
                    .collect::<store::Result<Vec<_>>>()?;
                json!({"observations": observations})
            }
            "storage_status" => storage::status(None)?,
            "host_status" => health::host_status()?,
            "universe_stats" => health::universe_stats()?,
            _ => return Err("Unsupported operation".into()),
        })
    })();
    match result {
        Ok(data) => json!({"ok":true,"observed_at":now(),"data":data}),
        Err(error) => json!({"ok":false,"observed_at":now(),"error":error.to_string()}),
    }
}

pub fn handle(db: &Connection, request: &Value) -> Value {
    let op = request
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or("");
    let result: Result<Value, Box<dyn std::error::Error>> = (|| {
        Ok(match op {
            "create"
            | "delete"
            | "clone"
            | "start"
            | "stop"
            | "pause"
            | "resume"
            | "resources"
            | "migration_preflight"
            | "migration_checkpoint"
            | "migration_authorize_transfer"
            | "migration_complete_transfer"
            | "migration_retire_source"
            | "migration_release"
            | "migration_abandon"
            | "migration_restore_local"
            | "migration_destination_preflight"
            | "migration_restore"
            | "migration_restore_abort" => lifecycle::execute(db, request)?,
            // Host-wide by design: the collector is the only operation that does not name one universe.
            "garbage_collect_plan" | "garbage_collect_apply" => collector::execute(db, request)?,
            "collection_retention_declare"
            | "collection_hold_declare"
            | "collection_hold_release"
            | "collection_status" => retention::execute(db, request)?,
            "manager_status"
            | "manager_decision"
            | "manager_observe"
            | "manager_vote_ledger_init"
            | "manager_vote_ledger_mark_unadmitted"
            | "manager_vote_ledger_readmit"
            | "manager_decision_propose" => manager::execute(db, request)?,
            "secret_declare" | "secret_remove" | "secret_status" => secrets::execute(db, request)?,
            "publisher_declare" | "publisher_start" | "publisher_stop" | "publisher_status"
            | "publisher_observed" => publisher::execute(db, request)?,
            "network_declare"
            | "network_undeclare"
            | "network_route_publish"
            | "network_route_withdraw"
            | "network_route_resume"
            | "network_reapply"
            | "network_status" => network::execute(db, request)?,
            "activation_require"
            | "activation_acquire"
            | "activation_renew"
            | "activation_release"
            | "activation_supersede"
            | "activation_status"
            | "activation_fence"
            | "activation_fence_preview" => activation::execute(db, request)?,
            "recovery_point_prepare"
            | "recovery_point_status"
            | "recovery_point_restore"
            | "recovery_point_promote"
            | "recovery_point_stage"
            | "recovery_point_discard"
            | "recovery_point_resume" => recovery_point::execute(db, request)?,
            "boot_restore" | "boot_restore_status" => boot_restore::execute(db, request)?,
            "migration_status" => migration::status(db, request)?,
            "volume_declare" | "volume_grow" => storage::execute(db, request)?,
            "storage_status" => storage::status(Some(db))?,
            "host_status" => health::host_status()?,
            "universe_stats" => health::universe_stats()?,
            "capabilities" => json!({
                "schemas": schema::all(),
                "schema_version": "podmesh-operation-schema/1",
                "version":option_env!("PODMESH_PACKAGE_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
                "operations":["capabilities","identity","inventory","observations","activation_require","activation_acquire","activation_renew","activation_release","activation_supersede","activation_status","activation_fence","activation_fence_preview","recovery_point_prepare","recovery_point_status","recovery_point_restore","recovery_point_promote","recovery_point_stage","recovery_point_discard","recovery_point_resume","create","delete","clone","start","stop","pause","resume","resources","storage_status","volume_declare","volume_grow","host_status","universe_stats"],
                "experimental_operations":["migration_preflight","migration_checkpoint","migration_status","migration_authorize_transfer",
                    "migration_complete_transfer","migration_retire_source","migration_release","migration_abandon","migration_restore_local",
                    "migration_destination_preflight","migration_restore","migration_restore_abort",
                    "garbage_collect_plan","garbage_collect_apply","collection_retention_declare","collection_hold_declare","collection_hold_release","collection_status","network_declare","network_undeclare","network_route_publish","network_route_withdraw","network_route_resume","network_reapply","network_status","boot_restore","boot_restore_status","manager_status","manager_decision","manager_observe","manager_vote_ledger_init","manager_vote_ledger_mark_unadmitted","manager_vote_ledger_readmit","manager_decision_propose","secret_declare","secret_remove","secret_status","publisher_declare","publisher_start","publisher_stop","publisher_status","publisher_observed"],
                "experimental_contracts":{
                    "migration_preflight":"read-only compatibility report bound to universe UUID, container ID, image ID, source and destination host UUIDs, optional migration_profile flat (default) or nested (Rule 11 outer preflight only); no reservation, suspension or artifact",
                    "migration_checkpoint":"source: migration_profile flat (default) runs fresh checks, durable reservation, checkpoint with the packaged podmesh-vzcriu runtime in its own scope, archive/manifest/hashes under the state directory; migration_profile nested assesses Rule 11 outer shape, records a durable reservation and preflight.json, reconciles inner Podman metadata for the kit counter fixture when present, assesses nested VFS store binding and records nested_vfs_store_binding.json, assesses the Rule 11 destination restore chain and records rule11_destination_restore_chain.json, captures the outer universe with the packaged runtime when preflight passes (nested_outer_checkpoint_plan.json, archive/manifest with nested sidecars), then reports the remaining gap for the two-host carry/restore/complete chain; never an authorization to restore",
                    "migration_status":"read-only reservation, fresh observation, artifact re-hash, transfer authorizations, restore claims, archived reservations and which recovery operations the observed state permits",
                    "migration_authorize_transfer":"source: checkpointed -> transfer_authorized after re-hashing the artifacts and observing the checkpointed source; authorization recorded first, then archive, manifest and handoff in outbox/<authorization_id>/",
                    "migration_complete_transfer":"source: inbox/<authorization_id>/outcome.json bound to this handoff; restored -> transferred, not_restored -> checkpointed with the authorization ended; any mismatch refused without state change",
                    "migration_retire_source":"source: from transferred, removes only the stopped, checkpointed reserved container; reservation and evidence kept",
                    "migration_release":"source: checkpointed or checkpoint_failed -> released, only when no transfer authorization was ever issued and the same reserved container is observed stopped; lifts the generic-operation gate and starts nothing",
                    "migration_abandon":"source: reserved, checkpointing, checkpoint_failed or checkpointed with the reserved container absent -> abandoned, only when no authorization was ever issued; artifacts kept and the universe UUID stays refused",
                    "migration_restore_local":"source: from released, resumes the checkpointed memory here from the checkpoint files Podman kept (same container ID) or, when they are gone, from the preserved archive; verified like a destination restore, then the reservation is archived",
                    "migration_destination_preflight":"destination, read-only: inbox handoff names this host; name, label, reservation and claims free; image, runtime, kernel, archive, manifest and space checked; nested source checkpoints additionally assess inbox sidecars against the manifest and record nested destination restore hooks",
                    "migration_restore":"destination: preflight, durable claim, restore of a private archive copy with the packaged runtime in its own scope; verified only when the universe runs restored and the CRIU restore log names the qualified runtime; nested profile additionally reconciles inner Podman metadata for the kit counter fixture and verifies counter continuity against the source checkpoint proof; records ownership and writes outbox/<authorization_id>/outcome.json",
                    "migration_restore_abort":"destination: never removes a running or verified universe; removes only a non-running container created by a held claim, or declines an unclaimed authorization, then records not_restored and writes the outcome. With the explicit reclaim_processes: true it first ends the processes it can prove belong to the failed attempt (membership of the container's own libpod or libpod-conmon cgroup, start time at or after the claim, both re-read immediately before the signal) and verifies that both cgroups disappeared; without it, surviving processes are reported and nothing is removed or signalled",
                    "restore_bound":"a restore attempt is watched while it runs: if it consumes more of the Podman graph root than its own preflight required, or that filesystem falls below the floor, its container cgroup is frozen (nothing is ended) and the transient scope is stopped",
                    "garbage_collect_plan":"host-wide and read-only: enumerates a bounded set of reservations and unresolved restore claims, their class, every proof fact observed and every blocker, and proposes an effect for each. No Podman mutation, no signal, no deletion, no state change beyond its own immutable run record. Age never justifies collection: proof does",
                    "garbage_collect_apply":"separately authorized: names the plan it applies, the candidates it may act on, and its own bounds (max_effects, max_runtime_reclaims, reclaim_processes). It repeats every proof immediately before each effect, stops at the first mismatch, and verifies each result from outside. Terminal reservation classes 1 and 2 become a collected reservation with a tombstone; a failed restore claim is delegated to migration_restore_abort. There is no timer and no artifact collection in this version",
                    "tombstone":"a collected universe UUID keeps refusing create and clone into that identity for good; a container proven absent at collection never regains ownership through its original creation. Only a verified handoff restore, or an explicit replacement procedure, gives that identity a meaning again",
                    "reservation":"a reservation or an unresolved restore claim blocks create, start, delete and clone for the universe; stop remains available, and a released or collected reservation blocks nothing",
                    "boot_restore":"host-wide, journaled, called at boot by a local unit under the operator's mandate or by an operator or agent: starts, at most once per boot each, the universes whose last journaled intent is to run, through the start gates, under the caller's authorization_ref; never a managed-network universe while the declaration is not effective, an epoch-gated one, a lease-gated one whose lease was not acquired or renewed during this boot, a universe with recovery points and no policy, a quarantined copy, a holding migration reservation or unresolved restore claim, or a universe with a capture still dumping or an unfinished live promotion recorded after its last intent",
                    "network_reapply":"host-wide, journaled, called at boot before boot_restore: re-applies the effects of this host's effective declaration that the kernel or Podman no longer shows (bridge, peer routes, NAT exemption) and withdraws every recorded /32 route whose kernel route is gone, with its alias, and every route row left resuming whatever the kernel shows; a /32 route is never re-applied from the ledger",
                    "network_route_resume":"host-wide, journaled (exclusive_resource): the recorded exclusive route and alias of a role this host still holds, put back in place after the carrier lost them (a stop, a restart, a roll at the same address): the row kept and marked resuming, the dead effects removed, the alias and route made again with the recorded ip, via and resource and verified; a failure or a crash leaves the row recorded for the next resume; only while the lease is live, held here, unsuperseded and was acquired or renewed during this boot, a universe runs at via, and the kernel holds no other route for the address; refused otherwise with a named reason (no_policy, no_recorded_route, route_incomplete, lease_not_entitled, lease_not_renewed_this_boot, declaration_not_effective, kernel_unknown, other_kernel_route, alias_unknown, no_carrier_at_via); an effective route answers already_effective; never changes the holder, acquires or renews",
                    "publisher_start":"every method of takeover proof is held until its eligible_after on this host's clock and accepted from it, and an expired one refused; the takeover proof is verified and recorded with the lease incarnation and boot it was verified under; without a proof, or with one refused, the start resumes under the recorded one (method resume_same_epoch, journaled with the original proof's identity) only while the lease is live, held here, unsuperseded, at the same epoch, generation and acquisition, in the same boot and under the same authority, key and quorum; refused otherwise with a named reason",
                    "authority_quorum":"a policy may name its authority as a quorum of replica keys (threshold a strict majority of 1 to 9 keys) instead of one key: acquisition, supersession and the takeover proof then take a certificate that many distinct keys of the quorum signed under the policy's digest, verified here with no network; a duplicate, unknown or malformed signature refuses the whole certificate by name; no permit is accepted, so the epoch screen moves forward on certificates only and never backwards; the authority set carries a serial its digest covers, one more at every change, and changes only under a certificate of the policy in place bound to that serial or the operator's re-declaration naming the digest it replaces; a single key is the 1-of-1 case and its documents and permits are accepted as before",
                    "publisher_startup_withdrawal":"not a request: the daemon's own journaled operation at startup, withdrawing, connector and mark, every declared publisher present without a live, unsuperseded lease held here",
                    "boot_restore_status":"read-only: this boot's passes, what a pass would decide now for every universe intended to run, and the operations a previous run of the service left pending"
                },
                "scope":"local rootful Podman; network-disabled universes created or cloned by this host's PodMesh journal; one request at a time",
                "contracts":{
                    "ownership":"delete, start, stop and clone sources require a verified create, clone, migration_restore or live recovery_point_promote in this host's journal for the same universe and container ID",
                    "recovery_point_prepare":"capture stopped (default) exports a universe already stopped (class quiescent); capture live checkpoints a running universe with the qualified private runtime and resumes it in place from the kept images (class memory-coherent): the universe is interrupted for the dump and the resume, about half a second on a small universe, and its memory continues; gated by the activation lease as a start is; refused for a universe with a network, mounts, a TTY, more than 1 GiB of memory or non-musl processes",
                    "recovery_point_stage":"a live point's archive held on this host, verified against its manifest, no container created: a memory checkpoint restores as running processes, so the copy is restored only by recovery_point_promote under the lease",
                    "recovery_point_resume":"brings a universe left stopped by a final live capture (capture live, resume false) back in place from the images it kept, under the lease gate; the way back when its promotion elsewhere did not happen",
                    "recovery_point_discard":"removes a staged point's archive and manifest from this host's inbox; refused for a point promoted here",
                    "start":"observe_seconds 0-30 (default 2); reports running or not running as observed, with exit code when not running",
                    "stop":"timeout_seconds 0-300 and on_timeout kill|leave_running are required; kill lets podman escalate to SIGKILL after the timeout, leave_running only sends the stop signal",
                    "pause":"freezes every process of a running universe; memory and address stay; never gated by the lease; a paused universe answers none_already_paused",
                    "resume":"thaws a paused universe; gated by the activation lease exactly as start; a running universe answers none_already_running",
                    "resources":"memory_bytes (32 MiB to this host's total) and/or cpus (0.1 to this host's cores); applied to the live cgroup and read back from the kernel when running, kept for the next start otherwise",
                    "retry":"a verified operation ID returns its historical result with a fresh observation; pending or failed operations are re-evaluated; no cancellation operation",
                    "clone":"stopped, mount-free source through a committed snapshot image"
                }
            }),
            "identity" => {
                json!({"host_uuid":db.query_row("SELECT value FROM metadata WHERE key='host_uuid'",[],|r|r.get::<_,String>(0))?})
            }
            "inventory" => json!({"containers":inventory()?,"store":"default rootful Podman"}),
            "observations" => {
                let mut stmt = db.prepare(
                    "SELECT id,observed_at,operation FROM observations ORDER BY id DESC LIMIT 20",
                )?;
                let rows = stmt.query_map([], |r| {
                    Ok(json!({"id":r.get::<_,i64>(0)?,"observed_at":r.get::<_,i64>(1)?,"operation":r.get::<_,String>(2)?}))
                })?;
                json!({"observations":rows.collect::<Result<Vec<_>,_>>()?})
            }
            _ => return Err("Unsupported operation".into()),
        })
    })();
    let response = match result {
        Ok(data) => json!({"ok":true,"observed_at":now(),"data":data}),
        Err(e) => {
            let mut failure = json!({"ok":false,"observed_at":now(),"error":e.to_string()});
            // Failures after an effect was attempted carry the freshly observed state.
            if let Some(f) = e.downcast_ref::<lifecycle::Failure>() {
                failure["details"] = f.details.clone();
            }
            failure
        }
    };
    if let Err(e) = db.execute(
        "INSERT INTO observations(observed_at,operation,result) VALUES(?1,?2,?3)",
        params![now() as i64, op, response.to_string()],
    ) {
        return json!({"ok":false,"error":format!("Observation persistence failed: {e}")});
    }
    response
}

#[cfg(test)]
mod api_store_tests {
    use super::*;
    use store::{Integrity, Result as StoreResult, Row, Transaction};

    /// The local API test does not need a server: this adapter exercises the non-SQLite
    /// `NodeStore` branch while SQLite supplies the contract implementation underneath.
    struct MariaDbAdapter(SqliteStore);

    impl DurableStore for MariaDbAdapter {
        fn engine(&self) -> Engine {
            Engine::Mariadb
        }

        fn execute(&mut self, sql: &str, params: &[Stored]) -> StoreResult<u64> {
            self.0.execute(sql, params)
        }

        fn execute_batch(&mut self, sql: &str) -> StoreResult<()> {
            self.0.execute_batch(sql)
        }

        fn query(&mut self, sql: &str, params: &[Stored]) -> StoreResult<Vec<Row>> {
            self.0.query(sql, params)
        }

        fn transaction(&mut self) -> StoreResult<Box<dyn Transaction + '_>> {
            self.0.transaction()
        }

        fn integrity_check(&mut self) -> StoreResult<Integrity> {
            self.0.integrity_check()
        }

        fn tables(&mut self) -> StoreResult<Vec<String>> {
            self.0.tables()
        }
    }

    #[test]
    fn a_durable_node_serves_reads_and_round_trips_a_lifecycle_write() {
        lifecycle::prepare_scratch(
            &std::env::temp_dir().join(format!("podmesh-lifecycle-store-test-{}", std::process::id())),
        )
        .unwrap();
        let mut sqlite = SqliteStore::open_in_memory().unwrap();
        migrations::apply(&mut sqlite).unwrap();
        sqlite
            .execute(
                "INSERT INTO metadata(`key`, value) VALUES(?, ?)",
                &[
                    Stored::from("host_uuid"),
                    Stored::from("host-from-durable-store"),
                ],
            )
            .unwrap();
        let mut node = NodeStore::Durable(Box::new(MariaDbAdapter(sqlite)));

        let capabilities = node.handle(&json!({"operation": "capabilities"}));
        assert_eq!(capabilities["ok"], true);
        assert_eq!(capabilities["data"]["store_engine"], "mariadb");
        assert!(capabilities["data"]["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|operation| operation == "capabilities"));

        let identity = node.handle(&json!({"operation": "identity"}));
        assert_eq!(identity["ok"], true);
        assert_eq!(identity["data"]["host_uuid"], "host-from-durable-store");

        let deletion = json!({
            "operation": "delete",
            "operation_id": "durable-delete",
            "universe_uuid": "16926159-bf59-4537-8f6e-5cfea52540ea",
            "authorization_ref": "test"
        });
        let written = node.handle(&deletion);
        assert_eq!(written["ok"], true, "{written}");
        assert_eq!(written["data"]["absent"], true);

        let replayed = node.handle(&deletion);
        assert_eq!(replayed["ok"], true, "{replayed}");
        assert_eq!(replayed["data"]["replayed"], true);
        assert_eq!(replayed["data"]["original_result"]["absent"], true);

        let refused = node.handle(&json!({"operation": "migration_checkpoint"}));
        assert_eq!(refused["ok"], false);
        assert_eq!(refused["error_code"], "store_engine_unsupported");
        assert_eq!(refused["operation"], "migration_checkpoint");
    }
}
