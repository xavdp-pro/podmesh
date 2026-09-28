//! Secrets a universe is given at creation, outside every image layer (Codex's finding B2).
//!
//! A replica's configuration carries its pair keys; baked into an image it would leave with the
//! image -- published, exported, cached, shared. Here the bytes never enter an image, this
//! journal, or the API line: the operator (or the agent, over root SSH) places the file under
//! `inbox/secrets/<source>` of the state directory, root-only, and `secret_declare` hands it to
//! Podman's secret store under a name, records the name with the digest and size -- never the
//! content -- and removes the inbox copy. `create` then mounts the secret into the universe at
//! the target path the caller names, root-only (0600), and labels the container with the names
//! and targets so that a promotion from a recovery point can ask for the same secrets by name.
//! `secret_remove` refuses while any container carries the name. `secret_status` lists names,
//! digests and presence in Podman's store, never content.
//!
//! What this does not decide: the durable operator copy of the secret (kept wherever the
//! operator keeps it, outside PodMesh, never touched here) and Podman's own store at rest
//! (root-only files on the host: the laboratory's accepted boundary, said in the contract).
use crate::lifecycle as lc;
use crate::store::{DurableStore, Value as Stored};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::os::unix::fs::{MetadataExt, PermissionsExt};

type Error = Box<dyn std::error::Error>;

pub const LABEL_SECRETS: &str = "io.podmesh.secrets";
const MAX_BYTES: u64 = 65536;

pub fn ensure_schema(db: &Connection) -> Result<(), Error> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS secrets(
            name TEXT PRIMARY KEY,
            sha256 TEXT NOT NULL,
            bytes INTEGER NOT NULL,
            declared_at INTEGER NOT NULL,
            operation_id TEXT NOT NULL,
            authorization_ref TEXT NOT NULL,
            removed_at INTEGER);",
    )?;
    if !crate::store::catalog::connection::has_column(db, "secrets", "state")? {
        db.execute_batch("ALTER TABLE secrets ADD COLUMN state TEXT NOT NULL DEFAULT 'effective';")?;
    }
    Ok(())
}

/// The digest of what Podman's store holds under a name, read back through the store itself.
fn store_digest(name: &str) -> Option<String> {
    let out = std::process::Command::new("podman").args(["secret", "inspect", "--showsecret", "--format", "{{.SecretData}}", name]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    // The template output ends with one newline Podman adds; the content itself may end with one too.
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let content = text.strip_suffix('\n').unwrap_or(&text);
    crate::migration::sha256_bytes(content.as_bytes()).ok()
}

/// A secret's state (Codex, P1): `declaring` recorded with the intended digest before Podman's
/// store is touched, `effective` once the store's content hashes to it, `removing` before the
/// removal, gone after. Names are immutable: another content needs another name. Reconciliation
/// finishes or undoes whatever is not effective: a `declaring` secret whose store content hashes
/// to the intended digest becomes effective, one whose content differs or is absent is removed
/// from the store with its row; a `removing` one is removed.
pub(crate) fn reconcile(db: &Connection) -> Result<Vec<Value>, Error> {
    ensure_schema(db)?;
    let mut s = db.prepare("SELECT name,sha256,state FROM secrets WHERE removed_at IS NULL AND state!='effective'")?;
    let rows: Vec<(String, String, String)> = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
    let mut report = vec![];
    for (name, digest, state) in rows {
        match state.as_str() {
            "declaring" => {
                if in_store(&name) == Some(true) && store_digest(&name).as_deref() == Some(digest.as_str()) {
                    db.execute("UPDATE secrets SET state='effective' WHERE name=?1", [&name])?;
                    report.push(json!({"name": name, "was": "declaring", "now": "effective", "why": "the store holds the intended content"}));
                } else {
                    if in_store(&name) == Some(true) {
                        lc::podman(lc::QUICK, &["secret", "rm", &name])?;
                    }
                    if in_store(&name) != Some(false) {
                        report.push(json!({"name": name, "was": "declaring", "now": "declaring", "error": "the store could not be cleared or asked"}));
                        continue;
                    }
                    db.execute("DELETE FROM secrets WHERE name=?1", [&name])?;
                    report.push(json!({"name": name, "was": "declaring", "now": "gone", "why": "the store did not hold the intended content"}));
                }
            }
            "removing" => {
                if in_store(&name) == Some(true) {
                    lc::podman(lc::QUICK, &["secret", "rm", &name])?;
                }
                if in_store(&name) != Some(false) {
                    report.push(json!({"name": name, "was": "removing", "now": "removing", "error": "the store could not be cleared or asked"}));
                    continue;
                }
                db.execute("DELETE FROM secrets WHERE name=?1", [&name])?;
                report.push(json!({"name": name, "was": "removing", "now": "gone"}));
            }
            other => report.push(json!({"name": name, "state": other, "error": "unknown state"})),
        }
    }
    Ok(report)
}

fn inbox_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("PODMESH_STATE_DIR").unwrap_or("/var/lib/podmesh".into())).join("inbox").join("secrets")
}

/// Podman's store, asked directly: the names it holds now.
fn in_store(name: &str) -> Option<bool> {
    let out = std::process::Command::new("podman").args(["secret", "exists", name]).output().ok()?;
    Some(out.status.success())
}

/// A declared, present secret: what `create` requires before mounting it.
pub(crate) fn declared(db: &Connection, name: &str) -> Result<(), Error> {
    ensure_schema(db)?;
    let row: Option<String> = db
        .query_row("SELECT sha256 FROM secrets WHERE name=?1 AND removed_at IS NULL AND state='effective'", [name], |r| r.get(0))
        .optional()?;
    if row.is_none() {
        return Err(format!("secret {name} is not declared on this host (secret_declare)").into());
    }
    match in_store(name) {
        Some(true) => Ok(()),
        Some(false) => Err(format!("secret {name} is declared here but absent from Podman's store; declare it again").into()),
        None => Err(format!("Podman's secret store could not be asked about {name}; refusing on an unknown state").into()),
    }
}

/// The secrets a create request names: `[{name, target}]`, validated, as Podman arguments and
/// as the label that records names and targets only.
pub(crate) fn mounts(db: &Connection, request: &Value) -> Result<(Vec<String>, Option<String>), Error> {
    let Some(list) = request.get("secrets") else { return Ok((vec![], None)) };
    let list = list.as_array().ok_or("secrets must be a list of {name, target}")?;
    let mut args = vec![];
    let mut label = vec![];
    for entry in list {
        let name = lc::text(entry, "name")?;
        lc::token(name)?;
        let target = lc::text(entry, "target")?;
        if !target.starts_with('/') || target.contains("..") || target.contains(',') || target.ends_with('/') {
            return Err(format!("secret target {target} must be an absolute file path without '..' or ','").into());
        }
        declared(db, name)?;
        args.push("--secret".to_string());
        args.push(format!("source={name},type=mount,target={target},mode=0600,uid=0,gid=0"));
        label.push(format!("{name}:{target}"));
    }
    if label.is_empty() {
        return Ok((vec![], None));
    }
    Ok((args, Some(format!("{LABEL_SECRETS}={}", label.join(";")))))
}

/// The `secrets` list a container's label describes, for a promotion that re-attaches them.
pub(crate) fn from_label(labels: &Value) -> Value {
    match labels[LABEL_SECRETS].as_str() {
        None => Value::Null,
        Some(s) => json!(s
            .split(';')
            .filter_map(|pair| pair.split_once(':').map(|(n, t)| json!({"name": n, "target": t})))
            .collect::<Vec<_>>()),
    }
}

pub fn execute(db: &Connection, request: &Value) -> Result<Value, Error> {
    let operation = lc::text(request, "operation")?;
    lc::ensure_schema(db)?;
    ensure_schema(db)?;
    if operation == "secret_status" {
        return view(db);
    }
    lc::journaled(db, request, |db| perform(db, request))
}

fn view(db: &Connection) -> Result<Value, Error> {
    let mut s = db.prepare("SELECT name,sha256,bytes,declared_at,state FROM secrets WHERE removed_at IS NULL ORDER BY name")?;
    let rows: Vec<Value> = s
        .query_map([], |r| {
            let name: String = r.get(0)?;
            Ok(json!({"name": name, "sha256": r.get::<_, String>(1)?, "bytes": r.get::<_, i64>(2)?, "declared_at": r.get::<_, i64>(3)?,
                      "state": r.get::<_, String>(4)?, "in_store": in_store(&name)}))
        })?
        .collect::<Result<_, _>>()?;
    Ok(json!({"secrets": rows, "inbox": inbox_dir(),
              "scope": "names, digests and presence in Podman's store on this host; never content. The durable copy is the operator's, outside PodMesh"}))
}

fn perform(db: &Connection, request: &Value) -> Result<Value, Error> {
    let operation = lc::text(request, "operation")?;
    let id = lc::text(request, "operation_id")?;
    let reference = lc::text(request, "authorization_ref")?;
    let name = lc::text(request, "name")?;
    lc::token(name)?;
    match operation {
        "secret_declare" => {
            let source = lc::text(request, "source")?;
            lc::token(source)?;
            let path = inbox_dir().join(source);
            let meta = std::fs::metadata(&path).map_err(|e| format!("no secret at {}: {e}", path.display()))?;
            if !meta.is_file() {
                return Err(format!("{} is not a regular file", path.display()).into());
            }
            if meta.uid() != 0 || meta.permissions().mode() & 0o077 != 0 {
                return Err(format!("{} must be owned by root with no group or other permission; nothing was read", path.display()).into());
            }
            if meta.len() == 0 || meta.len() > MAX_BYTES {
                return Err(format!("a secret is 1 to {MAX_BYTES} bytes; {} holds {}", path.display(), meta.len()).into());
            }
            let bytes = std::fs::read(&path)?;
            let digest = crate::migration::sha256_bytes(&bytes)?;
            let existing: Option<(String, String)> = db
                .query_row("SELECT sha256,state FROM secrets WHERE name=?1 AND removed_at IS NULL", [name], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            if let Some((previous, state)) = existing {
                if state == "effective" && previous == digest && in_store(name) == Some(true) {
                    std::fs::remove_file(&path)?;
                    return Ok(json!({"name": name, "sha256": digest, "bytes": bytes.len(), "inbox_copy_removed": true, "in_store": true, "already_declared": true}));
                }
                if state == "effective" {
                    return Err(format!("secret {name} is already declared with another content; names are immutable: declare the new content under a new name and switch the universe to it").into());
                }
                return Err(format!("secret {name} is in state {state}; reconciliation finishes it first").into());
            }
            // The intent first, durable: the name and the digest it must hold; then the store.
            // A name removed earlier may be declared again (its row is history): the intent replaces it.
            db.execute(
                "INSERT INTO secrets(name,sha256,bytes,declared_at,operation_id,authorization_ref,state) VALUES(?1,?2,?3,?4,?5,?6,'declaring')
                 ON CONFLICT(name) DO UPDATE SET sha256=excluded.sha256, bytes=excluded.bytes, declared_at=excluded.declared_at,
                   operation_id=excluded.operation_id, authorization_ref=excluded.authorization_ref, state='declaring', removed_at=NULL",
                params![name, digest, bytes.len() as i64, crate::now() as i64, id, reference],
            )?;
            let mut child = std::process::Command::new("podman")
                .args(["secret", "create", name, "-"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()?;
            {
                use std::io::Write;
                child.stdin.take().ok_or("secret stdin")?.write_all(&bytes)?;
            }
            let out = child.wait_with_output()?;
            if !out.status.success() {
                db.execute("DELETE FROM secrets WHERE name=?1 AND state='declaring'", [name])?;
                return Err(format!("podman secret create: {}", String::from_utf8_lossy(&out.stderr).trim()).into());
            }
            // From here a failure compensates: the store entry goes and the intent with it. A crash
            // leaves the intent `declaring`, and reconciliation finishes or undoes it from the store.
            let settle = |db: &Connection| -> Result<(), Error> {
                lc::fault("secret-after-store")?;
                // Verified from the store itself: the bytes it holds hash to the intent.
                if store_digest(name).as_deref() != Some(digest.as_str()) {
                    return Err(format!("secret {name}: the store does not hold the intended content after its creation").into());
                }
                lc::fault("secret-before-effective")?;
                db.execute("UPDATE secrets SET state='effective' WHERE name=?1", [name])?;
                Ok(())
            };
            if let Err(err) = settle(db) {
                let _ = lc::podman(lc::QUICK, &["secret", "rm", name]);
                let gone = in_store(name) == Some(false);
                if gone {
                    db.execute("DELETE FROM secrets WHERE name=?1 AND state='declaring'", [name])?;
                }
                return Err(format!("{err}; compensation: store entry {}", if gone { "removed, nothing is declared" } else { "NOT removed; reconciliation will" }).into());
            }
            // The inbox copy has served; the durable copy is the operator's.
            std::fs::remove_file(&path)?;
            Ok(json!({"name": name, "sha256": digest, "bytes": bytes.len(), "inbox_copy_removed": true, "in_store": true,
                      "note": "the content is in Podman's root-only store and nowhere in PodMesh"}))
        }
        "secret_remove" => {
            let row: Option<String> = db.query_row("SELECT sha256 FROM secrets WHERE name=?1 AND removed_at IS NULL", [name], |r| r.get(0)).optional()?;
            if row.is_none() {
                return Err(format!("secret {name} is not declared on this host").into());
            }
            // A container that carries the name still mounts it: refused, whatever its state.
            let inventory: Value = serde_json::from_str(&lc::podman(lc::QUICK, &["ps", "--all", "--format", "json"])?)?;
            let users: Vec<String> = inventory
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter(|c| c["Labels"][LABEL_SECRETS].as_str().is_some_and(|l| l.split(';').any(|pair| pair.split_once(':').map(|(n, _)| n) == Some(name))))
                        .filter_map(|c| c["Names"][0].as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            if !users.is_empty() {
                return Err(format!("secret {name} is carried by {}; delete those universes first", users.join(", ")).into());
            }
            db.execute("UPDATE secrets SET state='removing' WHERE name=?1", [name])?;
            if in_store(name) == Some(true) {
                lc::podman(lc::QUICK, &["secret", "rm", name])?;
            }
            lc::fault("secret-remove-after-store")?;
            if in_store(name) != Some(false) {
                return Err(format!("secret {name} is still in Podman's store after its removal, or the store could not be asked; its removal is finished by reconciliation").into());
            }
            db.execute("DELETE FROM secrets WHERE name=?1", [name])?;
            Ok(json!({"name": name, "removed": true, "in_store": false}))
        }
        _ => Err("Unsupported secret operation".into()),
    }
}

/// The same three operations through the store contract, for a journal that is not a file.
///
/// `secret_status` reads without journaling, exactly as [`execute`] does. `secret_declare` and
/// `secret_remove` are journaled by operation ID with the same replay rule as the connection
/// path: a verified ID returns its persisted result marked replayed and never runs again, any
/// other ID runs and is recorded verified or failed with an attempt row either way.
///
/// Every statement uses `?` placeholders bound by position, so the same text runs on both
/// engines. The declaring intent is recorded with a delete then an insert inside one
/// transaction, which is atomic on both engines.
pub fn execute_store(store: &mut dyn DurableStore, request: &Value) -> Result<Value, Error> {
    let operation = lc::text(request, "operation")?;
    if operation == "secret_status" {
        return view_store(store);
    }
    journaled_store(store, request, |store| perform_store(store, request))
}

fn view_store(store: &mut dyn DurableStore) -> Result<Value, Error> {
    let rows = store.query(
        "SELECT name, sha256, bytes, declared_at, state FROM secrets WHERE removed_at IS NULL ORDER BY name",
        &[],
    )?;
    let mut listed = Vec::with_capacity(rows.len());
    for row in &rows {
        let name = row.text(0)?.to_string();
        listed.push(json!({"name": name, "sha256": row.text(1)?, "bytes": row.integer(2)?, "declared_at": row.integer(3)?,
                          "state": row.text(4)?, "in_store": in_store(&name)}));
    }
    Ok(json!({"secrets": listed, "inbox": inbox_dir(),
              "scope": "names, digests and presence in Podman's store on this host; never content. The durable copy is the operator's, outside PodMesh"}))
}

fn stored_optional_text(value: &Stored) -> Result<Option<String>, Error> {
    match value {
        Stored::Null => Ok(None),
        Stored::Text(text) => Ok(Some(text.clone())),
        Stored::Blob(bytes) => Ok(Some(String::from_utf8_lossy(bytes).into_owned())),
        other => Err(format!("column is {}, not text", other.kind()).into()),
    }
}

fn journaled_store(
    store: &mut dyn DurableStore,
    request: &Value,
    run: impl FnOnce(&mut dyn DurableStore) -> Result<Value, Error>,
) -> Result<Value, Error> {
    let id = lc::text(request, "operation_id")?;
    lc::token(id)?;
    let canonical = request.to_string();
    if let Some(row) = store.query_one("SELECT request, status, result FROM operations WHERE id = ?", &[Stored::from(id)])? {
        let saved = row.text(0)?.to_string();
        let status = row.text(1)?.to_string();
        let result = stored_optional_text(row.value(2)?)?;
        if saved != canonical {
            return Err("Operation ID already belongs to a different request".into());
        }
        if status == "verified" {
            let stored = result.ok_or("Missing persisted result")?;
            let mut original: Value = serde_json::from_str(&stored)?;
            original["replayed"] = json!(true);
            original["historical"] = json!(true);
            original["notice"] = json!("this is the result persisted when the operation was verified, not current state; a replay repeats no effect");
            return Ok(original);
        }
    } else {
        store.execute(
            "INSERT INTO operations(id, request, status) VALUES(?, ?, ?)",
            &[Stored::from(id), Stored::from(canonical.clone()), Stored::from("pending")],
        )?;
    }
    store.execute(
        "INSERT INTO operation_attempts(operation_id, started_at) VALUES(?, ?)",
        &[Stored::from(id), Stored::from(crate::now() as i64)],
    )?;
    let attempt = store
        .query_one("SELECT id FROM operation_attempts WHERE operation_id = ? ORDER BY id DESC LIMIT 1", &[Stored::from(id)])?
        .ok_or("Missing operation attempt")?
        .integer(0)?;
    let outcome = run(store);
    let finished = crate::now() as i64;
    match &outcome {
        Ok(result) => {
            store.execute(
                "UPDATE operations SET status = ?, result = ? WHERE id = ?",
                &[Stored::from("verified"), Stored::from(result.to_string()), Stored::from(id)],
            )?;
            store.execute(
                "UPDATE operation_attempts SET finished_at = ?, outcome = ? WHERE id = ?",
                &[Stored::from(finished), Stored::from("verified"), Stored::from(attempt)],
            )?;
        }
        Err(error) => {
            let record = json!({"error": error.to_string()}).to_string();
            store.execute(
                "UPDATE operations SET status = ?, result = ? WHERE id = ?",
                &[Stored::from("failed"), Stored::from(record.clone()), Stored::from(id)],
            )?;
            store.execute(
                "UPDATE operation_attempts SET finished_at = ?, outcome = ?, detail = ? WHERE id = ?",
                &[Stored::from(finished), Stored::from("failed"), Stored::from(record), Stored::from(attempt)],
            )?;
        }
    }
    outcome
}

fn perform_store(store: &mut dyn DurableStore, request: &Value) -> Result<Value, Error> {
    let operation = lc::text(request, "operation")?;
    let id = lc::text(request, "operation_id")?;
    let reference = lc::text(request, "authorization_ref")?;
    let name = lc::text(request, "name")?;
    lc::token(name)?;
    match operation {
        "secret_declare" => {
            let source = lc::text(request, "source")?;
            lc::token(source)?;
            let path = inbox_dir().join(source);
            let meta = std::fs::metadata(&path).map_err(|e| format!("no secret at {}: {e}", path.display()))?;
            if !meta.is_file() {
                return Err(format!("{} is not a regular file", path.display()).into());
            }
            if meta.uid() != 0 || meta.permissions().mode() & 0o077 != 0 {
                return Err(format!("{} must be owned by root with no group or other permission; nothing was read", path.display()).into());
            }
            if meta.len() == 0 || meta.len() > MAX_BYTES {
                return Err(format!("a secret is 1 to {MAX_BYTES} bytes; {} holds {}", path.display(), meta.len()).into());
            }
            let bytes = std::fs::read(&path)?;
            let digest = crate::migration::sha256_bytes(&bytes)?;
            let existing = store.query_one("SELECT sha256, state FROM secrets WHERE name = ? AND removed_at IS NULL", &[Stored::from(name)])?;
            if let Some(row) = existing {
                let previous = row.text(0)?.to_string();
                let state = row.text(1)?.to_string();
                if state == "effective" && previous == digest && in_store(name) == Some(true) {
                    std::fs::remove_file(&path)?;
                    return Ok(json!({"name": name, "sha256": digest, "bytes": bytes.len(), "inbox_copy_removed": true, "in_store": true, "already_declared": true}));
                }
                if state == "effective" {
                    return Err(format!("secret {name} is already declared with another content; names are immutable: declare the new content under a new name and switch the universe to it").into());
                }
                return Err(format!("secret {name} is in state {state}; reconciliation finishes it first").into());
            }
            // The intent first, durable: the name and the digest it must hold; then the store.
            // A name removed earlier may be declared again (its row is history): the intent replaces it.
            {
                let mut tx = store.transaction()?;
                tx.execute("DELETE FROM secrets WHERE name = ?", &[Stored::from(name)])?;
                tx.execute(
                    "INSERT INTO secrets(name, sha256, bytes, declared_at, operation_id, authorization_ref, state) VALUES(?, ?, ?, ?, ?, ?, ?)",
                    &[
                        Stored::from(name),
                        Stored::from(digest.clone()),
                        Stored::from(bytes.len() as i64),
                        Stored::from(crate::now() as i64),
                        Stored::from(id),
                        Stored::from(reference),
                        Stored::from("declaring"),
                    ],
                )?;
                tx.commit()?;
            }
            let mut child = std::process::Command::new("podman")
                .args(["secret", "create", name, "-"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()?;
            {
                use std::io::Write;
                child.stdin.take().ok_or("secret stdin")?.write_all(&bytes)?;
            }
            let out = child.wait_with_output()?;
            if !out.status.success() {
                store.execute("DELETE FROM secrets WHERE name = ? AND state = ?", &[Stored::from(name), Stored::from("declaring")])?;
                return Err(format!("podman secret create: {}", String::from_utf8_lossy(&out.stderr).trim()).into());
            }
            // From here a failure compensates: the store entry goes and the intent with it. A crash
            // leaves the intent `declaring`, and reconciliation finishes or undoes it from the store.
            let settle = |store: &mut dyn DurableStore| -> Result<(), Error> {
                lc::fault("secret-after-store")?;
                // Verified from the store itself: the bytes it holds hash to the intent.
                if store_digest(name).as_deref() != Some(digest.as_str()) {
                    return Err(format!("secret {name}: the store does not hold the intended content after its creation").into());
                }
                lc::fault("secret-before-effective")?;
                store.execute("UPDATE secrets SET state = ? WHERE name = ?", &[Stored::from("effective"), Stored::from(name)])?;
                Ok(())
            };
            if let Err(err) = settle(store) {
                let _ = lc::podman(lc::QUICK, &["secret", "rm", name]);
                let gone = in_store(name) == Some(false);
                if gone {
                    store.execute("DELETE FROM secrets WHERE name = ? AND state = ?", &[Stored::from(name), Stored::from("declaring")])?;
                }
                return Err(format!("{err}; compensation: store entry {}", if gone { "removed, nothing is declared" } else { "NOT removed; reconciliation will" }).into());
            }
            // The inbox copy has served; the durable copy is the operator's.
            std::fs::remove_file(&path)?;
            Ok(json!({"name": name, "sha256": digest, "bytes": bytes.len(), "inbox_copy_removed": true, "in_store": true,
                      "note": "the content is in Podman's root-only store and nowhere in PodMesh"}))
        }
        "secret_remove" => {
            let held = store.query_one("SELECT sha256 FROM secrets WHERE name = ? AND removed_at IS NULL", &[Stored::from(name)])?;
            if held.is_none() {
                return Err(format!("secret {name} is not declared on this host").into());
            }
            // A container that carries the name still mounts it: refused, whatever its state.
            let inventory: Value = serde_json::from_str(&lc::podman(lc::QUICK, &["ps", "--all", "--format", "json"])?)?;
            let users: Vec<String> = inventory
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter(|c| c["Labels"][LABEL_SECRETS].as_str().is_some_and(|l| l.split(';').any(|pair| pair.split_once(':').map(|(n, _)| n) == Some(name))))
                        .filter_map(|c| c["Names"][0].as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            if !users.is_empty() {
                return Err(format!("secret {name} is carried by {}; delete those universes first", users.join(", ")).into());
            }
            store.execute("UPDATE secrets SET state = ? WHERE name = ?", &[Stored::from("removing"), Stored::from(name)])?;
            if in_store(name) == Some(true) {
                lc::podman(lc::QUICK, &["secret", "rm", name])?;
            }
            lc::fault("secret-remove-after-store")?;
            if in_store(name) != Some(false) {
                return Err(format!("secret {name} is still in Podman's store after its removal, or the store could not be asked; its removal is finished by reconciliation").into());
            }
            store.execute("DELETE FROM secrets WHERE name = ?", &[Stored::from(name)])?;
            Ok(json!({"name": name, "removed": true, "in_store": false}))
        }
        _ => Err("Unsupported secret operation".into()),
    }
}

#[cfg(test)]
mod secrets_store_tests {
    use super::*;
    use crate::store::{migrations, sqlite::SqliteStore};

    static ENV_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn temp_state(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("podmesh-secrets-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("inbox").join("secrets")).unwrap();
        dir
    }

    fn with_state_dir(name: &str) -> (std::sync::MutexGuard<'static, ()>, std::path::PathBuf, Option<String>) {
        let guard = ENV_SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_state(name);
        let previous = std::env::var("PODMESH_STATE_DIR").ok();
        std::env::set_var("PODMESH_STATE_DIR", &dir);
        (guard, dir, previous)
    }

    fn restore_state_dir(previous: Option<String>, dir: &std::path::Path) {
        match previous {
            Some(value) => std::env::set_var("PODMESH_STATE_DIR", value),
            None => std::env::remove_var("PODMESH_STATE_DIR"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    fn memory() -> SqliteStore {
        let mut store = SqliteStore::open_in_memory().unwrap();
        migrations::apply(&mut store).unwrap();
        store
    }

    fn insert_secret(
        store: &mut dyn DurableStore,
        name: &str,
        sha: &str,
        bytes: i64,
        declared_at: i64,
        operation: &str,
        authorization: &str,
        state: &str,
    ) {
        store
            .execute(
                "INSERT INTO secrets(name, sha256, bytes, declared_at, operation_id, authorization_ref, state) VALUES(?, ?, ?, ?, ?, ?, ?)",
                &[
                    Stored::from(name),
                    Stored::from(sha),
                    Stored::from(bytes),
                    Stored::from(declared_at),
                    Stored::from(operation),
                    Stored::from(authorization),
                    Stored::from(state),
                ],
            )
            .unwrap();
    }

    #[test]
    fn store_status_matches_connection_on_the_same_journal() {
        let (_guard, dir, previous) = with_state_dir("status");
        let mut store = memory();
        let empty = json!({"operation": "secret_status"});
        let empty_store = execute_store(&mut store, &empty).unwrap();
        assert_eq!(empty_store["secrets"].as_array().unwrap().len(), 0);
        assert_eq!(empty_store["inbox"], json!(dir.join("inbox").join("secrets")));
        assert!(empty_store["scope"].as_str().unwrap().contains("never content"));
        insert_secret(&mut store, "alpha", "sha-alpha", 3, 100, "op-1", "auth-1", "effective");
        insert_secret(&mut store, "beta", "sha-beta", 5, 200, "op-2", "auth-1", "effective");
        let request = json!({"operation": "secret_status"});
        let via_store = execute_store(&mut store, &request).unwrap();
        let db = store.into_connection();
        let via_connection = execute(&db, &request).unwrap();
        assert_eq!(via_store, via_connection);
        let listed = via_store["secrets"].as_array().unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0]["name"], "alpha");
        assert_eq!(listed[1]["name"], "beta");
        for entry in listed {
            assert!(entry.get("content").is_none());
            assert_eq!(entry["sha256"].as_str().unwrap().starts_with("sha-"), true);
            assert!(entry.get("bytes").is_some());
            assert!(entry.get("state").is_some());
            assert!(entry.get("in_store").is_some());
        }
        restore_state_dir(previous, &dir);
    }

    #[test]
    fn store_declare_missing_inbox_fails_like_connection_and_journals_the_failure() {
        let (_guard, dir, previous) = with_state_dir("declare-missing");
        let mut store = memory();
        let request = json!({"operation": "secret_declare", "operation_id": "op-declare-missing-1",
            "authorization_ref": "auth-1", "name": "alpha", "source": "missing-file"});
        let store_err = execute_store(&mut store, &request).unwrap_err().to_string();
        let db = rusqlite::Connection::open_in_memory().unwrap();
        let connection_err = execute(&db, &request).unwrap_err().to_string();
        assert_eq!(store_err, connection_err);
        assert!(store_err.contains("no secret at"));
        let row = store
            .query_one("SELECT status FROM operations WHERE id = ?", &[Stored::from("op-declare-missing-1")])
            .unwrap()
            .expect("the failed declare is journaled");
        assert_eq!(row.text(0).unwrap(), "failed");
        let attempts = store
            .query("SELECT id FROM operation_attempts WHERE operation_id = ?", &[Stored::from("op-declare-missing-1")])
            .unwrap();
        assert_eq!(attempts.len(), 1);
        let again = execute_store(&mut store, &request).unwrap_err().to_string();
        assert_eq!(again, store_err);
        let attempts = store
            .query("SELECT id FROM operation_attempts WHERE operation_id = ?", &[Stored::from("op-declare-missing-1")])
            .unwrap();
        assert_eq!(attempts.len(), 2);
        let clash = json!({"operation": "secret_declare", "operation_id": "op-declare-missing-1",
            "authorization_ref": "auth-1", "name": "alpha", "source": "other-file"});
        let clash_err = execute_store(&mut store, &clash).unwrap_err().to_string();
        assert!(clash_err.contains("Operation ID already belongs to a different request"));
        restore_state_dir(previous, &dir);
    }

    #[test]
    fn store_remove_unknown_fails_like_connection_and_journals_the_failure() {
        let (_guard, dir, previous) = with_state_dir("remove-unknown");
        let mut store = memory();
        let request = json!({"operation": "secret_remove", "operation_id": "op-remove-unknown-1",
            "authorization_ref": "auth-1", "name": "ghost"});
        let store_err = execute_store(&mut store, &request).unwrap_err().to_string();
        let db = rusqlite::Connection::open_in_memory().unwrap();
        let connection_err = execute(&db, &request).unwrap_err().to_string();
        assert_eq!(store_err, connection_err);
        assert!(store_err.contains("is not declared on this host"));
        let row = store
            .query_one("SELECT status FROM operations WHERE id = ?", &[Stored::from("op-remove-unknown-1")])
            .unwrap()
            .expect("the failed remove is journaled");
        assert_eq!(row.text(0).unwrap(), "failed");
        restore_state_dir(previous, &dir);
    }

    #[test]
    fn a_verified_operation_replays_without_touching_the_store() {
        let (_guard, dir, previous) = with_state_dir("replay");
        let mut store = memory();
        let request = json!({"operation": "secret_declare", "operation_id": "op-replay-1",
            "authorization_ref": "auth-1", "name": "alpha", "source": "missing-file"});
        let canonical = request.to_string();
        let result = json!({"name": "alpha", "sha256": "abc", "bytes": 3});
        store
            .execute(
                "INSERT INTO operations(id, request, status, result) VALUES(?, ?, ?, ?)",
                &[Stored::from("op-replay-1"), Stored::from(canonical), Stored::from("verified"), Stored::from(result.to_string())],
            )
            .unwrap();
        let via_store = execute_store(&mut store, &request).unwrap();
        assert_eq!(via_store["replayed"], true);
        assert_eq!(via_store["historical"], true);
        assert_eq!(via_store["name"], "alpha");
        assert!(via_store["notice"].as_str().unwrap().contains("not current state"));
        let attempts = store
            .query("SELECT id FROM operation_attempts WHERE operation_id = ?", &[Stored::from("op-replay-1")])
            .unwrap();
        assert!(attempts.is_empty());
        let db = store.into_connection();
        let via_connection = execute(&db, &request).unwrap();
        assert_eq!(via_store, via_connection);
        let clash = json!({"operation": "secret_declare", "operation_id": "op-replay-1",
            "authorization_ref": "auth-1", "name": "alpha", "source": "other-file"});
        let clash_err = execute(&db, &clash).unwrap_err().to_string();
        assert!(clash_err.contains("Operation ID already belongs to a different request"));
        restore_state_dir(previous, &dir);
    }

    #[test]
    fn an_unsupported_secret_operation_is_refused_on_both_paths() {
        let (_guard, dir, previous) = with_state_dir("unsupported");
        let mut store = memory();
        let request = json!({"operation": "secret_rotate", "operation_id": "op-bad-1",
            "authorization_ref": "auth-1", "name": "alpha"});
        let store_err = execute_store(&mut store, &request).unwrap_err().to_string();
        let db = rusqlite::Connection::open_in_memory().unwrap();
        let connection_err = execute(&db, &request).unwrap_err().to_string();
        assert_eq!(store_err, connection_err);
        assert_eq!(store_err, "Unsupported secret operation");
        restore_state_dir(previous, &dir);
    }
}
