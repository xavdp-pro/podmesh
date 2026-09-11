mod lifecycle;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}
pub fn open_state(dir: &Path) -> Result<Connection, Box<dyn std::error::Error>> {
    fs::create_dir_all(dir)?;
    let db = Connection::open(dir.join("state.sqlite"))?;
    db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL); CREATE TABLE IF NOT EXISTS observations(id INTEGER PRIMARY KEY,observed_at INTEGER NOT NULL,operation TEXT NOT NULL,result TEXT NOT NULL);")?;
    let machine = fs::read_to_string("/etc/machine-id")?.trim().to_string();
    let stored: Option<String> = db
        .query_row("SELECT value FROM metadata WHERE key='machine_id'", [], |r| r.get(0))
        .optional()?;
    if let Some(previous) = stored {
        if previous != machine {
            return Err("State belongs to a different host; explicit identity adoption required".into());
        }
    }
    let tx = db.unchecked_transaction()?;
    tx.execute("INSERT OR IGNORE INTO metadata VALUES('machine_id',?1)", [&machine])?;
    let uuid = fs::read_to_string("/proc/sys/kernel/random/uuid")?;
    tx.execute("INSERT OR IGNORE INTO metadata VALUES('host_uuid',?1)", [uuid.trim()])?;
    tx.commit()?;
    lifecycle::prepare_scratch(&dir.join("podman-tmp"))?;
    Ok(db)
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
pub fn handle(db: &Connection, request: &Value) -> Value {
    let op = request.get("operation").and_then(Value::as_str).unwrap_or("");
    let result: Result<Value, Box<dyn std::error::Error>> = (|| {
        Ok(match op {
            "create" | "delete" | "clone" | "start" | "stop" => lifecycle::execute(db, request)?,
            "capabilities" => json!({
                "version":option_env!("PODMESH_PACKAGE_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
                "operations":["capabilities","identity","inventory","observations","create","delete","clone","start","stop"],
                "scope":"local rootful Podman; network-disabled universes created or cloned by this host's PodMesh journal; one request at a time",
                "contracts":{
                    "ownership":"delete, start, stop and clone sources require a verified journal creation for the same universe and container ID",
                    "start":"observe_seconds 0-30 (default 2); reports running or not running as observed, with exit code when not running",
                    "stop":"timeout_seconds 0-300 and on_timeout kill|leave_running are required; kill lets podman escalate to SIGKILL after the timeout, leave_running only sends the stop signal",
                    "retry":"a verified operation ID returns its historical result with a fresh observation; pending or failed operations are re-evaluated; no cancellation operation",
                    "clone":"stopped, mount-free source through a committed snapshot image"
                }
            }),
            "identity" => json!({"host_uuid":db.query_row("SELECT value FROM metadata WHERE key='host_uuid'",[],|r|r.get::<_,String>(0))?}),
            "inventory" => json!({"containers":inventory()?,"store":"default rootful Podman"}),
            "observations" => {
                let mut stmt = db.prepare("SELECT id,observed_at,operation FROM observations ORDER BY id DESC LIMIT 20")?;
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
