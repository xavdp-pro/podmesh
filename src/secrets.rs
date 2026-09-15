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
    let present: bool = db.query_row("SELECT COUNT(*) FROM pragma_table_info('secrets') WHERE name='state'", [], |r| Ok(r.get::<_, i64>(0)? > 0))?;
    if !present {
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
