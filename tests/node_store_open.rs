//! What `open_state` does now that the store profile decides which engine carries the journal.
//!
//! These run in a process of their own because opening a state directory claims this node's
//! process-wide scratch, migration and transfer directories: a unit test in the library would
//! claim them for whichever test happened to run first.
//!
//! Every case uses a state directory of its own under the system's scratch, and none of them
//! touches `/var/lib/podmesh`.
use podmesh::store::{migrations, Engine, MariadbConfig, StoreConfig};
use std::{fs, path::PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("podmesh-node-store-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

/// A node's identity is read from the machine it runs on. Where there is none there is nothing to
/// bind a journal to, and that is not a failure of this test.
fn machine_id() -> Option<String> {
    fs::read_to_string("/etc/machine-id").ok().map(|id| id.trim().to_string())
}

/// The default, which is the journal every node has today: `state.sqlite` under the state
/// directory, at the current schema version, bound to this machine, with the node's directories
/// prepared beside it. Nothing about this is new except that the schema arrives as migrations.
#[test]
fn a_state_directory_with_no_profile_opens_the_journal_it_always_had() {
    let Some(machine) = machine_id() else {
        eprintln!("skipped: this host has no /etc/machine-id to bind a journal to");
        return;
    };
    let dir = scratch("default");
    let profile = podmesh::store_profile(&dir).unwrap();
    assert_eq!(profile.engine, Engine::Sqlite);
    assert_eq!(profile.described(), format!("sqlite:{}", dir.join("state.sqlite").display()));

    let db = podmesh::open_state(&dir).unwrap();
    assert!(dir.join("state.sqlite").exists());

    let held: String = db.query_row("SELECT value FROM metadata WHERE key = 'machine_id'", [], |row| row.get(0)).unwrap();
    assert_eq!(held, machine);
    let host_uuid: String = db.query_row("SELECT value FROM metadata WHERE key = 'host_uuid'", [], |row| row.get(0)).unwrap();
    assert!(!host_uuid.is_empty());
    let version: i64 = db
        .query_row("SELECT version FROM store_schema WHERE name = 'node'", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, migrations::node_version());

    // Every table of the node's schema is there at the first open, not only the two the old open
    // created and the rest when a module first wrote to them.
    let carried: i64 = db
        .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'", [], |row| row.get(0))
        .unwrap();
    assert_eq!(carried as usize, migrations::node_tables().len() + 1, "the node's tables and store_schema");

    // Opened again, it is the same journal: nothing re-applied, nothing lost, the same host UUID.
    drop(db);
    let db = podmesh::open_state(&dir).unwrap();
    let again: String = db.query_row("SELECT value FROM metadata WHERE key = 'host_uuid'", [], |row| row.get(0)).unwrap();
    assert_eq!(again, host_uuid);
    drop(db);
    fs::remove_dir_all(&dir).unwrap();
}

/// A journal carried here by another host is refused, which is the rule that has always held and
/// now holds through the store contract rather than through SQLite's own statement.
#[test]
fn a_journal_from_another_machine_is_refused() {
    if machine_id().is_none() {
        eprintln!("skipped: this host has no /etc/machine-id to bind a journal to");
        return;
    }
    let dir = scratch("elsewhere");
    let db = podmesh::open_state(&dir).unwrap();
    db.execute("UPDATE metadata SET value = 'another-host' WHERE key = 'machine_id'", []).unwrap();
    drop(db);

    let refused = podmesh::open_state(&dir).unwrap_err().to_string();
    assert!(refused.contains("belongs to a different host"), "{refused}");
    fs::remove_dir_all(&dir).unwrap();
}

/// The profile is read from the state directory, and a profile that cannot be read is a refusal
/// rather than a silent return to the default.
#[test]
fn a_profile_beside_the_journal_is_read_and_a_broken_one_refused() {
    let dir = scratch("profile");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("store.json"),
        r#"{"store": {"engine": "sqlite", "sqlite": {"path": "/var/lib/podmesh/elsewhere.sqlite", "busy_timeout_ms": 250}}}"#,
    )
    .unwrap();
    let profile = podmesh::store_profile(&dir).unwrap();
    assert_eq!(profile.sqlite.path, PathBuf::from("/var/lib/podmesh/elsewhere.sqlite"));
    assert_eq!(profile.sqlite.busy_timeout, std::time::Duration::from_millis(250));

    fs::write(dir.join("store.json"), "{ not json").unwrap();
    let refused = podmesh::store_profile(&dir).unwrap_err().to_string();
    assert!(refused.contains("is not JSON"), "{refused}");
    fs::remove_dir_all(&dir).unwrap();
}

/// Fail closed. A profile that names MariaDB and not how to reach it never opens a file instead,
/// and a MariaDB journal that does open is not handed to callers that speak SQLite: the node
/// refuses by name until its operations are ported.
#[test]
fn a_mariadb_profile_never_falls_back_to_a_file() {
    let dir = scratch("mariadb");
    let incomplete = StoreConfig { engine: Engine::Mariadb, ..StoreConfig::for_state_dir(&dir) };
    let refused = podmesh::open_node_store(&dir, &incomplete).err().expect("an incomplete profile opens nothing").to_string();
    assert!(refused.contains("password_file"), "{refused}");
    assert!(!dir.join("state.sqlite").exists(), "an unopened MariaDB profile made a file");

    // Complete enough to be opened, and either this build has no backend or there is no server:
    // both are refusals, and neither is a journal in a file.
    let named = StoreConfig {
        engine: Engine::Mariadb,
        mariadb: MariadbConfig::from_dsn("mysql://podmesh-node@127.0.0.1:1/podmesh-node"),
        ..StoreConfig::for_state_dir(&dir)
    };
    let refused = podmesh::open_node_store(&dir, &named).err().expect("a MariaDB profile with no server opens nothing").to_string();
    assert!(refused.contains("mariadb feature") || refused.contains("could not be opened"), "{refused}");
    assert!(!dir.join("state.sqlite").exists(), "a MariaDB profile that did not open made a file");
    let _ = fs::remove_dir_all(&dir);
}

/// The other half of failing closed, against a server: the journal opens, its schema is applied,
/// and the node still refuses to serve from it because no operation reads it yet. The refusal
/// names the engine and the schema version, so an operator can see how far the cutover is.
#[cfg(feature = "mariadb")]
#[test]
fn a_mariadb_journal_opens_and_migrates_and_is_still_refused_by_the_operation_path() {
    let Some(mariadb) = MariadbConfig::from_environment() else {
        eprintln!("skipped: PODMESH_MARIADB_DSN names no MariaDB server");
        return;
    };
    if machine_id().is_none() {
        eprintln!("skipped: this host has no /etc/machine-id to bind a journal to");
        return;
    }
    let dir = scratch("mariadb-server");
    let config = StoreConfig { engine: Engine::Mariadb, mariadb, ..StoreConfig::for_state_dir(&dir) };

    let mut opened = podmesh::open_node_store(&dir, &config).unwrap();
    assert_eq!(opened.engine(), Engine::Mariadb);
    assert!(opened.connection().is_none());
    assert!(!dir.join("state.sqlite").exists(), "a MariaDB node wrote a file beside its state directory");

    // The identity binding ran on this engine, through the same statements SQLite takes.
    if let podmesh::NodeStore::Durable(store) = &mut opened {
        let held = store.query("SELECT value FROM metadata WHERE `key` = ?", &[podmesh::store::Value::from("machine_id")]).unwrap();
        assert_eq!(held[0].text(0).unwrap(), machine_id().unwrap());
        let version = podmesh::store::schema_version(store.as_mut(), migrations::NODE).unwrap();
        assert_eq!(version, Some(migrations::node_version()));
    }

    let refused = opened.into_connection().unwrap_err().to_string();
    assert!(refused.contains("store.engine is mariadb"), "{refused}");
    assert!(refused.contains("Set store.engine to sqlite"), "{refused}");

    // Left as it was found: this database is a test's, and the next run starts from nothing.
    let mut store = podmesh::store::open(&config).unwrap();
    for table in migrations::node_tables().iter().chain([podmesh::store::SCHEMA_TABLE].iter()) {
        store.execute_batch(&format!("DROP TABLE IF EXISTS `{table}`;")).unwrap();
    }
    let _ = fs::remove_dir_all(&dir);
}

/// The faults a caller decides on are the same names in a build with the backend and in one
/// without: a profile this build cannot open is `unsupported`, never a quiet fallback.
#[cfg(not(feature = "mariadb"))]
#[test]
fn a_build_without_the_backend_names_the_refusal() {
    let config = StoreConfig { engine: Engine::Mariadb, mariadb: MariadbConfig::from_dsn("mysql://u@127.0.0.1:1/d"), ..StoreConfig::default() };
    let refused = podmesh::store::open(&config).err().expect("a build with no backend opens no MariaDB store");
    assert_eq!(refused.fault, podmesh::store::Fault::Unsupported);
}

/// A versioned SQLite journal seeded with representative rows: the source the
/// offline migrate binary must copy exactly (row counts + logical SHA-256).
/// `secrets` follows the versioned 0006 schema (declared_at, not the ad-hoc
/// spike's mounted_at), so the copy path is exercised, not the refusal path.
#[test]
fn a_seeded_versioned_journal_is_the_migrate_copy_source() {
    let Some(_) = machine_id() else {
        eprintln!("skipped: this host has no /etc/machine-id to bind a journal to");
        return;
    };
    let dir = scratch("seeded");
    let db = podmesh::open_state(&dir).unwrap();
    db.execute(
        "INSERT INTO secrets(name, sha256, bytes, declared_at, operation_id, authorization_ref, removed_at, state) VALUES('s1', 'ab', 3, 1, 'op1', 'auth1', NULL, 'effective'), ('s2', 'cd', 4, 2, 'op2', 'auth2', NULL, 'effective')",
        [],
    )
    .unwrap();
    db.execute(
        "INSERT INTO observations(observed_at, operation, result) VALUES(1, 'op1', 'ok'), (2, 'op2', 'ok')",
        [],
    )
    .unwrap();
    let secrets: i64 = db.query_row("SELECT COUNT(*) FROM secrets", [], |row| row.get(0)).unwrap();
    assert_eq!(secrets, 2);
    let version: i64 = db
        .query_row("SELECT version FROM store_schema WHERE name = 'node'", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, migrations::node_version());
    drop(db);
    eprintln!("seeded versioned journal: {}", dir.join("state.sqlite").display());
}
