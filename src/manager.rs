//! The agent's typed door to a manager universe's control socket.
//!
//! A manager universe (`packaging/podmesh-manager/universe` in the web tree) runs the frozen
//! resident behind one Unix socket at a path only PID 1 of that universe can reach, and the
//! resident accepts an observation only from a peer whose UID is the configured writer. Nothing
//! outside the universe can reach that socket -- which is the point -- and until now the only
//! writer inside was the entrypoint recording each boot. This module is the ONE door PodMesh
//! offers from the host: two typed operations, carried over the same root-only API as every
//! other, each with `authorization_ref` as provenance, reaching the socket through the
//! container's own mount namespace (`/proc/<pid>/root/...`) from inside its PID namespace: the
//! resident checks the connecting peer's credentials, and a peer whose PID is not visible from
//! the universe is refused whatever its UID (measured in the lab on 2026-09-15: the same request
//! accepted from inside, refused from the host, accepted again from the host under `nsenter -p`).
//! So the daemon relays each request through a copy of itself entered into that namespace with
//! `nsenter` (util-linux, on every Debian host). The agent never sees the socket,
//! and PodMesh adds nothing to the resident's protocol: it forwards exactly one request it built
//! itself from the fields the agent named, and returns the resident's own reply beside the facts
//! it observed.
//!
//! - `manager_status` (read-only): the resident's bounded live diagnostic, never facts.
//! - `manager_observe` (journaled): append one observation in a scope this replica owns. The
//!   resident refuses a scope it does not own, a subject or value out of bounds, a peer that is
//!   not the writer; PodMesh refuses before that a universe that is not here, not running, or
//!   carrying no control socket at the contract path. The PodMesh operation ID is the resident's
//!   operation ID, so a request the journal re-evaluates after a crash between the resident's
//!   append and the journal's record is a replay for the resident too, and appends nothing twice.
//!
//! What this does not decide: whether an observation is exclusive to the governor (it is not:
//! every replica appends in its own scopes and replication carries them), and who may hold
//! `authorization_ref` (provenance, recorded, not verified, as everywhere in PodMesh).
use crate::lifecycle as lc;
type Error = Box<dyn std::error::Error>;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

/// The contract path of the control socket inside a manager universe.
pub const CONTROL_SOCKET: &str = "run/podmesh-manager/control.sock";
/// The resident's reply ceiling, and this side's.
const REPLY_LIMIT: usize = 32768;
const CONTROL_TIMEOUT: Duration = Duration::from_secs(15);

pub fn execute(db: &Connection, request: &Value) -> Result<Value, Error> {
    let operation = lc::text(request, "operation")?;
    let uuid = lc::text(request, "universe_uuid")?;
    lc::token(uuid)?;
    lc::ensure_schema(db)?;
    match operation {
        "manager_status" => {
            let (door, facts) = locate(uuid)?;
            let reply = control(&door, &json!({"operation": "status"}))?;
            Ok(json!({"universe_uuid": uuid, "container": facts, "resident_status": reply,
                      "scope": "the resident's bounded live diagnostic, read through the universe's own mount namespace; never its facts, which are read from the store"}))
        }
        "manager_observe" => lc::journaled(db, request, |_| observe(request, uuid)),
        other => Err(format!("Unknown manager operation {other}").into()),
    }
}

/// Where the socket is, and what was observed of the universe on the way: here, running, with a
/// PID, and the contract path present in its root. Each absence is its own refusal.
fn locate(uuid: &str) -> Result<(Door, Value), Error> {
    let name = format!("podmesh-{uuid}");
    let Some(c) = lc::inspect(&name)? else {
        return Err("manager operation refused: no such universe on this host".into());
    };
    if c["State"]["Running"] != json!(true) {
        return Err("manager operation refused: the universe is not running; the control socket exists only while its resident does".into());
    }
    let pid = c["State"]["Pid"].as_i64().filter(|p| *p > 0).ok_or("manager operation refused: the running universe reports no PID")?;
    let socket = std::path::PathBuf::from(format!("/proc/{pid}/root/{CONTROL_SOCKET}"));
    if !socket.exists() {
        return Err(format!("manager operation refused: the universe carries no control socket at the manager contract path /{CONTROL_SOCKET}; it is not a manager universe, or its resident has not created it").into());
    }
    let facts = json!({"container_id": c["Id"], "pid": pid, "image": c["Image"],
                       "network_profile": c["Config"]["Labels"][crate::network::LABEL_PROFILE]});
    Ok((Door { pid, socket }, facts))
}

/// The universe's PID and the socket path through its root: what the relay needs.
struct Door {
    pid: i64,
    socket: std::path::PathBuf,
}

fn observe(request: &Value, uuid: &str) -> Result<Value, Error> {
    let id = lc::text(request, "operation_id")?;
    let scope = lc::text(request, "scope")?;
    let subject = lc::text(request, "subject")?;
    let value = lc::text(request, "value")?;
    // The resident's own bounds, checked here first so that a refusal costs no connection and
    // says which field; the resident checks them again, which is its job, not a duplication.
    for (field, v) in [("operation_id", id), ("subject", subject)] {
        if v.is_empty() || v.len() > 128 || !v.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':')) {
            return Err(format!("{field} must be 1 to 128 safe ASCII characters for the resident").into());
        }
    }
    if scope.is_empty() || scope.len() > 128 || scope.starts_with('/') || scope.ends_with('/')
        || scope.split('/').any(|s| s.is_empty() || s == "." || s == ".." || !s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':')))
    {
        return Err("scope must be a bounded safe hierarchical token (segments of safe ASCII separated by /)".into());
    }
    if value.is_empty() || value.len() > 4096 {
        return Err("value must be nonempty and at most 4096 bytes".into());
    }
    let (door, facts) = locate(uuid)?;
    let reply = control(&door, &json!({"operation": "append_observation", "operation_id": id, "scope": scope, "subject": subject, "value": value}))?;
    if let Some(error) = reply.get("error").and_then(Value::as_str) {
        return Err(format!("the resident refused the observation: {error}").into());
    }
    Ok(json!({"universe_uuid": uuid, "container": facts, "scope": scope, "subject": subject, "resident_reply": reply,
              "note": "appended in this replica's own scope and carried by replication; not an exclusive effect, and not gated by the activation lease"}))
}

/// One request on the resident's private protocol, carried by a copy of this daemon entered
/// into the universe's PID namespace (`nsenter -t <pid> -p`), so that the resident sees a peer
/// it can identify. The relay's stdout is the reply; a relay that fails says why on stderr.
fn control(door: &Door, request: &Value) -> Result<Value, Error> {
    let exe = std::env::current_exe()?;
    let mut child = std::process::Command::new("nsenter")
        .args(["-t", &door.pid.to_string(), "-p", "--"])
        .arg(&exe)
        .arg("control-relay")
        .arg(&door.socket)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("the relay into the universe's PID namespace could not start: {e}"))?;
    child.stdin.take().ok_or("relay stdin")?.write_all(request.to_string().as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(format!("the control relay failed: {}", String::from_utf8_lossy(&out.stderr).trim()).into());
    }
    if out.stdout.len() > REPLY_LIMIT {
        return Err("the resident's reply exceeded the protocol ceiling; refused rather than truncated".into());
    }
    serde_json::from_slice(&out.stdout).map_err(|_| {
        format!("the resident's reply is not JSON: {:?}", String::from_utf8_lossy(&out.stdout[..out.stdout.len().min(120)])).into()
    })
}

/// The relay itself (`podmeshd control-relay <socket>`): stdin to the socket, written whole, the
/// write side shut, the reply read to end of stream within the ceiling and written to stdout.
pub fn control_relay(socket: &str) -> Result<(), Error> {
    let mut request = Vec::new();
    std::io::stdin().read_to_end(&mut request)?;
    let mut stream = UnixStream::connect(socket).map_err(|e| format!("the control socket refused the connection: {e}"))?;
    stream.set_read_timeout(Some(CONTROL_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_TIMEOUT))?;
    stream.write_all(&request)?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut reply = Vec::new();
    stream.take((REPLY_LIMIT + 1) as u64).read_to_end(&mut reply)?;
    std::io::stdout().write_all(&reply)?;
    Ok(())
}
