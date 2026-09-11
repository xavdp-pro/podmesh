//! Local managed-container operations. No implicit image pulls or network access.
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{fs, os::unix::fs::PermissionsExt, path::{Path, PathBuf}, process::Command, sync::OnceLock};
type Error = Box<dyn std::error::Error>;

const UNIVERSE: &str = "io.podmesh.universe";
const CREATION: &str = "io.podmesh.creation-operation";
const SNAPSHOT_FOR: &str = "io.podmesh.snapshot-for";
const SNAPSHOT_OPERATION: &str = "io.podmesh.snapshot-operation";
const SNAPSHOT_SOURCE: &str = "io.podmesh.snapshot-source-container";
const SNAPSHOT_REPOSITORY: &str = "localhost/podmesh-clone:";
// Podman states in which no container process can write the root filesystem.
const STOPPED: [&str; 3] = ["created", "exited", "stopped"];
// Seconds. The service handles one request at a time, so these also bound queueing.
const QUICK: u32 = 30;
const COMMIT: u32 = 300;

fn text<'a>(r: &'a Value, key: &str) -> Result<&'a str, Error> {
    r.get(key).and_then(Value::as_str).filter(|v| !v.is_empty()).ok_or_else(|| format!("Missing {key}").into())
}
fn token(value: &str) -> Result<(), Error> {
    if value.len() > 80 || !value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err("Identifier must contain 1-80 ASCII letters, digits or hyphens".into());
    }
    Ok(())
}
fn is_uuid(v: &str) -> bool {
    v.len()==36 && v.chars().enumerate().all(|(i,c)| if [8,13,18,23].contains(&i){c=='-'}else{c.is_ascii_hexdigit()})
}
// Podman temporary files for this service. A killed commit leaves its layer copy behind,
// so PodMesh owns the directory and empties it at startup and before each commit.
static SCRATCH: OnceLock<PathBuf> = OnceLock::new();
pub fn prepare_scratch(dir: &Path) -> Result<(), Error> {
    SCRATCH.get_or_init(|| dir.to_path_buf());
    empty_scratch()
}
fn empty_scratch() -> Result<(), Error> {
    let dir = SCRATCH.get().ok_or("Podman scratch directory not prepared")?;
    if dir.exists() { fs::remove_dir_all(dir)?; }
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
fn podman(timeout: u32, args: &[&str]) -> Result<String, Error> {
    // GNU timeout bounds this process group; no shell evaluates caller input.
    let limit = timeout.to_string();
    let scratch = SCRATCH.get().ok_or("Podman scratch directory not prepared")?;
    let out = Command::new("/usr/bin/timeout").env("TMPDIR", scratch).args(["--signal=TERM", "--kill-after=5", &limit, "/usr/bin/podman"]).args(args).output()?;
    if !out.status.success() { return Err(format!("Podman operation failed or timed out: {}", String::from_utf8_lossy(&out.stderr).trim()).into()); }
    Ok(String::from_utf8(out.stdout)?)
}
fn label<'a>(c: &'a Value, key: &str) -> Option<&'a str> { c["Config"]["Labels"][key].as_str() }
fn inspect(name: &str) -> Result<Option<Value>, Error> {
    let all: Value = serde_json::from_str(&podman(QUICK, &["ps", "--all", "--format", "json"])?)?;
    let exists = all.as_array().ok_or("Invalid inventory")?.iter().any(|c| c["Names"].as_array().map(|n| n.iter().any(|v| v.as_str()==Some(name))).unwrap_or(false));
    if !exists { return Ok(None); }
    let data: Value = serde_json::from_str(&podman(QUICK, &["container", "inspect", name])?)?;
    Ok(Some(data[0].clone()))
}
fn images() -> Result<Vec<Value>, Error> {
    let all: Value = serde_json::from_str(&podman(QUICK, &["images", "--all", "--format", "json"])?)?;
    Ok(all.as_array().ok_or("Invalid image inventory")?.clone())
}
fn image_id(i: &Value) -> &str { i["Id"].as_str().unwrap_or("").trim_start_matches("sha256:") }
fn names(i: &Value) -> Vec<&str> { i["Names"].as_array().map(|n| n.iter().filter_map(Value::as_str).collect()).unwrap_or_default() }
fn stopped(c: &Value) -> Result<(), Error> {
    let status = c["State"]["Status"].as_str().unwrap_or("unknown");
    if !STOPPED.contains(&status) { return Err(format!("Container state {status} is not stopped").into()); }
    Ok(())
}
/// A clone source must be a universe this host's journal recorded as created or cloned,
/// still carried by the same container. A matching label alone is not enough.
fn recorded(db: &Connection, c: &Value, uuid: &str) -> Result<(), Error> {
    let operation = label(c, CREATION).ok_or("Clone source has no creation operation")?;
    let row: Option<(String, Option<String>)> = db.query_row("SELECT request,result FROM operations WHERE id=?1 AND status='verified'", [operation], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    let (request, result) = row.ok_or("Clone source is not recorded as a verified PodMesh universe on this host")?;
    let request: Value = serde_json::from_str(&request)?;
    let result: Value = serde_json::from_str(&result.ok_or("Missing persisted result")?)?;
    let kind = request["operation"].as_str().unwrap_or("");
    if !["create","clone"].contains(&kind) || request["universe_uuid"].as_str()!=Some(uuid) || result["container_id"]!=c["Id"] {
        return Err("Clone source container does not match its recorded creation".into());
    }
    Ok(())
}
pub fn execute(db: &Connection, request: &Value) -> Result<Value, Error> {
    let operation = text(request, "operation")?;
    let id = text(request, "operation_id")?; token(id)?;
    let uuid = text(request, "universe_uuid")?;
    if !is_uuid(uuid) { return Err("Invalid universe UUID".into()); }
    text(request,"authorization_ref")?;
    if !["create","delete","clone"].contains(&operation) {return Err("Unsupported lifecycle operation".into());}
    // Reject malformed requests before they reserve an operation ID.
    let source = if operation=="clone" {
        let source=text(request,"source_uuid")?;
        if !is_uuid(source) {return Err("Invalid source UUID".into());}
        if source==uuid {return Err("A clone requires a new universe UUID".into());}
        source
    } else {""};
    db.execute_batch("CREATE TABLE IF NOT EXISTS operations(id TEXT PRIMARY KEY, request TEXT NOT NULL, status TEXT NOT NULL, result TEXT);")?;
    let canonical = request.to_string();
    let previous: Option<(String,String,Option<String>)> = db.query_row("SELECT request,status,result FROM operations WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    if let Some((saved,status,result))=previous {
        if saved!=canonical {return Err("Operation ID already belongs to a different request".into());}
        if status=="verified" {return Ok(json!({"replayed":true,"original_result":serde_json::from_str::<Value>(&result.ok_or("Missing persisted result")?)?}));}
    } else {db.execute("INSERT INTO operations VALUES(?1,?2,'pending',NULL)",params![id,canonical])?;}
    let name = format!("podmesh-{uuid}");
    let existing=inspect(&name)?;
    if let Some(ref c)=existing {
        if label(c,UNIVERSE)!=Some(uuid) {return Err("Target is not managed by PodMesh".into());}
    }
    let result=if operation=="clone" {
        clone(db,id,uuid,source,&name,existing)?
    } else if operation=="create" {
        let image=text(request,"image")?;
        // An immutable local image ID is mandatory for this first version.
        if image.len()!=71 || !image.starts_with("sha256:") || !image[7..].bytes().all(|c|c.is_ascii_hexdigit()) {return Err("Use a full local sha256 image ID".into());}
        let command=request["command"].as_array().ok_or("command must be an array")?;
        let command:Vec<&str>=command.iter().map(|v|v.as_str().ok_or("command items must be strings")).collect::<Result<_,_>>()?;
        if command.is_empty(){return Err("Explicit command required".into());}
        if let Some(ref c)=existing {
            if label(c,CREATION)!=Some(id) {return Err("Universe already exists under another creation operation".into());}
        } else {
            let label=format!("{UNIVERSE}={uuid}");
            let provenance=format!("{CREATION}={id}");
            let mut args=vec!["create","--pull=never","--network=none","--name",&name,"--label",&label,"--label",&provenance,image];
            args.extend(command);
            podman(QUICK,&args)?;
        }
        let c=inspect(&name)?.ok_or("Created container not observable")?;
        json!({"status":"verified","state":c["State"]["Status"],"universe_uuid":uuid,"container_id":c["Id"],"network":"none","started":false})
    } else {
        if let Some(c)=existing {
            stopped(&c).map_err(|e| format!("Refusing to delete: {e}; stop it explicitly first"))?;
            // Do not force removal and do not remove volumes.
            podman(QUICK,&["rm",&name])?;
        }
        if inspect(&name)?.is_some(){return Err("Container still present after removal".into());}
        let (removed,retained)=remove_snapshots(uuid)?;
        json!({"status":"verified","universe_uuid":uuid,"absent":true,"volumes":"retained","snapshot_images_removed":removed,"snapshot_images_retained":retained})
    };
    db.execute("UPDATE operations SET status='verified', result=?2 WHERE id=?1",params![id,result.to_string()])?;
    Ok(result)
}
/// Clone a stopped, mount-free universe: commit its root filesystem into a snapshot image
/// tagged by operation ID, then create a new network-disabled container from that image.
/// A retry reuses a snapshot committed by an interrupted attempt of the same operation.
fn clone(db: &Connection, id: &str, uuid: &str, source: &str, name: &str, existing: Option<Value>) -> Result<Value, Error> {
    let reference=format!("{SNAPSHOT_REPOSITORY}{id}");
    let mut reused=false;
    if let Some(ref c)=existing {
        if label(c,CREATION)!=Some(id) {return Err("Clone target already belongs to another operation".into());}
    } else {
        let prior=images()?.into_iter().find(|i| names(i).contains(&reference.as_str()));
        let image=if let Some(snapshot)=prior {
            let l=&snapshot["Labels"];
            if l[SNAPSHOT_FOR].as_str()!=Some(uuid) || l[SNAPSHOT_OPERATION].as_str()!=Some(id) || l[UNIVERSE].as_str()!=Some(source) {
                return Err("Snapshot reference exists with different provenance; refusing to reuse it".into());
            }
            reused=true;
            image_id(&snapshot).to_string()
        } else {
            let source_name=format!("podmesh-{source}");
            let before=inspect(&source_name)?.ok_or("Clone source not found")?;
            if label(&before,UNIVERSE)!=Some(source) {return Err("Clone source is not managed by PodMesh".into());}
            recorded(db,&before,source)?;
            stopped(&before).map_err(|e| format!("Clone source must be stopped for a coherent filesystem snapshot: {e}"))?;
            if before["Mounts"].as_array().map(|m|!m.is_empty()).unwrap_or(true){return Err("Volume and bind mount cloning is not supported yet".into());}
            let source_id=before["Id"].as_str().ok_or("Missing source container identity")?;
            let changes=[format!("LABEL {SNAPSHOT_FOR}={uuid}"),format!("LABEL {SNAPSHOT_OPERATION}={id}"),format!("LABEL {SNAPSHOT_SOURCE}={source_id}")];
            // Never pause: a source started concurrently is detected below, not frozen.
            empty_scratch()?;
            podman(COMMIT,&["commit","--pause=false","--change",&changes[0],"--change",&changes[1],"--change",&changes[2],&source_name,&reference])?;
            let after=inspect(&source_name)?;
            let unchanged=after.as_ref().is_some_and(|a| a["Id"]==before["Id"] && a["State"]["StartedAt"]==before["State"]["StartedAt"] && stopped(a).is_ok());
            if !unchanged {
                let _=podman(QUICK,&["image","rm",&reference]);
                return Err("Clone source was started or replaced during the snapshot; snapshot discarded".into());
            }
            let snapshot=images()?.into_iter().find(|i| names(i).contains(&reference.as_str())).ok_or("Snapshot image not observable after commit")?;
            image_id(&snapshot).to_string()
        };
        let label=format!("{UNIVERSE}={uuid}");
        let provenance=format!("{CREATION}={id}");
        podman(QUICK,&["create","--pull=never","--network=none","--name",name,"--label",&label,"--label",&provenance,&image])?;
    }
    // Verify the observed clone, whether created now or by an interrupted attempt.
    let c=inspect(name)?.ok_or("Clone not observable")?;
    let image=c["Image"].as_str().unwrap_or("").trim_start_matches("sha256:").to_string();
    let snapshot=images()?.into_iter().find(|i| image_id(i)==image).ok_or("Clone snapshot image not observable")?;
    let l=&snapshot["Labels"];
    if label(&c,CREATION)!=Some(id) || l[SNAPSHOT_FOR].as_str()!=Some(uuid) || l[SNAPSHOT_OPERATION].as_str()!=Some(id) || l[UNIVERSE].as_str()!=Some(source) {
        return Err("Clone provenance does not match the operation".into());
    }
    if c["HostConfig"]["NetworkMode"].as_str()!=Some("none") || c["Mounts"].as_array().map(|m|!m.is_empty()).unwrap_or(true) {
        return Err("Clone configuration does not match the supported scope".into());
    }
    Ok(json!({"status":"verified","universe_uuid":uuid,"source_uuid":source,"container_id":c["Id"],"state":c["State"]["Status"],
        "source_container_id":l[SNAPSHOT_SOURCE],"snapshot_image":image,"snapshot_reference":reference,"snapshot_reused":reused,
        "started":false,"network":"none","scope":"stopped container root filesystem; no volumes or bind mounts"}))
}
/// Remove snapshot images committed for this universe. Never forced: an image still used
/// by a container or by a dependent image is retained and reported.
fn remove_snapshots(uuid: &str) -> Result<(Vec<String>, Vec<Value>), Error> {
    let (mut removed, mut retained) = (vec![], vec![]);
    for i in images()?.iter().filter(|i| i["Labels"][SNAPSHOT_FOR].as_str()==Some(uuid)) {
        let id=image_id(i).to_string();
        let tags=names(i);
        if tags.iter().any(|n| !n.starts_with(SNAPSHOT_REPOSITORY)) {
            retained.push(json!({"image":id,"reason":"image has names outside the PodMesh snapshot repository"}));
            continue;
        }
        let targets=if tags.is_empty() {vec![id.as_str()]} else {tags};
        let mut args=vec!["image","rm"]; args.extend(targets);
        match podman(QUICK,&args) {
            Ok(_)=>removed.push(id),
            Err(e)=>retained.push(json!({"image":id,"reason":e.to_string()})),
        }
    }
    // Removing a tag succeeds even when a dependent image keeps the image; report what remains.
    let remaining=images()?;
    removed.retain(|id| {
        let present=remaining.iter().any(|i| image_id(i)==id.as_str());
        if present { retained.push(json!({"image":id,"reason":"untagged but still present; a dependent image or container uses it"})); }
        !present
    });
    Ok((removed, retained))
}
