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
//! - `manager_decision` (read-only, V3-5): the resident's answer to `decision_read` for one resource:
//!   the current decision of the replicas with its quorum certificate, or what is missing. What it
//!   carries is not trusted here: a certificate is verified by the activation and publisher operations
//!   it is delivered to, with the keys of this host's own policy. The host's decision follow tick
//!   (`packaging/podmesh-decision-follow`) reads it and delivers the certificate; the door gives the
//!   resident no way to act on this host.
//! - `manager_observe` (journaled): append one observation in a scope this replica owns. The
//!   resident refuses a scope it does not own, a subject or value out of bounds, a peer that is
//!   not the writer; PodMesh refuses before that a universe that is not here, not running, or
//!   carrying no control socket at the contract path. The PodMesh operation ID is the resident's
//!   operation ID, so a request the journal re-evaluates after a crash between the resident's
//!   append and the journal's record is a replay for the resident too, and appends nothing twice.
//!
//! THE REPLICA'S HOST STATE (V3-5). A manager replica that votes keeps its signing key and its signing
//! ledger in a directory its host provides, outside the universe's state, and reads the host's
//! machine-id from a read-only mount: no recovery point, restore, clone or migration of the universe
//! carries or rewinds them. `create` with `manager_host_state: <name>` gives the universe exactly three
//! bind mounts, derived here and nowhere else: `<state>/manager-host/<name>/votes` read-write at
//! `/run/podmesh-host/votes`, `<state>/manager-host/<name>/evidence` read-only at
//! `/run/podmesh-host/evidence` (the operator's readmission evidence, which the replica cannot write),
//! and the host's `/etc/machine-id` read-only at `/run/podmesh-host/machine-id`. The caller names no
//! path. The directories are made private (0700) at the first create and never removed by PodMesh: a
//! roll that deletes and re-creates the replica under the same name finds its key and ledger again.
//! A universe with mounts is refused by live captures and migrations, and a stopped capture exports
//! its root filesystem only, without them.
//!
//! What this does not decide: whether an observation is exclusive to the active manager (it is
//! not: every replica appends in its own scopes and replication carries them), and who may hold
//! `authorization_ref` (provenance, recorded, not verified, as everywhere in PodMesh).
use crate::lifecycle as lc;
type Error = Box<dyn std::error::Error>;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

/// Where the manager replicas' host state lives: `<state>/manager-host`, set when the state opens.
static HOST_STATE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
/// The label that names a universe's host state.
pub const LABEL_HOST_STATE: &str = "io.podmesh.manager-host-state";

/// Records where the manager replicas' host state lives, under this node's state directory.
pub fn prepare_host_state(state_dir: &std::path::Path) {
    let _ = HOST_STATE.set(state_dir.join("manager-host"));
}

/// The Podman arguments of a manager replica's host state `name`, and the label that records it: the
/// vote directory read-write, the evidence directory and the host's machine-id read-only, each at its
/// fixed path in the universe. The two directories are made if absent, private, and must be real
/// directories (no symlink): what the replica is bound to is decided here. They sit in this node's
/// state directory, which only the service's user can write.
pub(crate) fn host_state_mounts(name: &str) -> Result<(Vec<String>, String), Error> {
    lc::token(name)?;
    if name.is_empty() {
        return Err("manager_host_state must name the replica's host state".into());
    }
    let root = HOST_STATE.get().ok_or("the manager host state directory was not prepared")?;
    host_state_mounts_in(root, name)
}

fn host_state_mounts_in(root: &std::path::Path, name: &str) -> Result<(Vec<String>, String), Error> {
    use std::os::unix::fs::PermissionsExt;
    let base = root.join(name);
    let mut paths = Vec::new();
    for sub in ["votes", "evidence"] {
        let dir = base.join(sub);
        std::fs::create_dir_all(&dir)?;
        for p in [root, base.as_path(), dir.as_path()] {
            let m = std::fs::symlink_metadata(p)?;
            if !m.is_dir() {
                return Err(format!("{} must be a directory, not a symlink", p.display()).into());
            }
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700))?;
        }
        let text = dir.to_str().ok_or("the host state path is not UTF-8")?.to_string();
        if text.contains(',') {
            return Err("the host state path must not contain a comma".into());
        }
        paths.push(text);
    }
    let args = vec![
        "--mount".to_string(),
        format!("type=bind,src={},dst=/run/podmesh-host/votes", paths[0]),
        "--mount".to_string(),
        format!("type=bind,src={},dst=/run/podmesh-host/evidence,ro=true", paths[1]),
        "--mount".to_string(),
        "type=bind,src=/etc/machine-id,dst=/run/podmesh-host/machine-id,ro=true".to_string(),
    ];
    Ok((args, format!("{LABEL_HOST_STATE}={name}")))
}

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
            // The store's size on disk, read through the resident's root: the exchange audit table is never
            // compacted, so this is the leading indicator that replication will degrade (2026-09-16).
            let store_bytes = facts["pid"].as_i64().and_then(|pid| std::fs::metadata(format!("/proc/{pid}/root/var/lib/podmesh-manager/manager.sqlite")).ok()).map(|m| m.len());
            Ok(json!({"universe_uuid": uuid, "container": facts, "resident_status": reply, "store_bytes": store_bytes,
                      "scope": "the resident's bounded live diagnostic, read through the universe's own mount namespace; never its facts, which are read from the store"}))
        }
        "manager_decision" => {
            let relayed = decision_request(request)?;
            let (door, facts) = locate(uuid)?;
            let reply = control(&door, &relayed)?;
            Ok(json!({"universe_uuid": uuid, "container": facts, "resource": relayed["resource"], "resident_reply": reply,
                      "scope": "this host's replica's reading of the replicas' decision, relayed as it answered; a certificate in it is verified by the operation it is delivered to, never here"}))
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
    let container_id = c["Id"].as_str().ok_or("the container reports no identity")?.to_string();
    // The PID is bound to the container it was read from: the process must sit in that container's
    // cgroup, and its start time is recorded so that the same PID reused by another process is told
    // apart. Checked again right before the relay is spawned and after it returns (Codex, I3).
    let started = identity(pid, &container_id).ok_or_else(|| format!("manager operation refused: PID {pid} is not a process of container {container_id}; the universe changed under the operation"))?;
    let facts = json!({"container_id": container_id, "pid": pid, "process_started": started, "image": c["Image"],
                       "network_profile": c["Config"]["Labels"][crate::network::LABEL_PROFILE]});
    Ok((Door { pid, socket, container_id, started }, facts))
}

/// The universe's PID, its container's identity and the process's start time, and the socket
/// path through its root: what the relay needs and what binds it to one incarnation.
struct Door {
    pid: i64,
    socket: std::path::PathBuf,
    container_id: String,
    started: String,
}

/// The start time of a PID that sits in the container's cgroup, from /proc; `None` when the PID
/// is gone, belongs to another cgroup, or cannot be read -- every one a reason to refuse.
fn identity(pid: i64, container_id: &str) -> Option<String> {
    let cgroup = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    if !cgroup.contains(&format!("libpod-{container_id}")) {
        return None;
    }
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Field 22 counts after the parenthesised command name, which may itself hold spaces.
    let after = &stat[stat.rfind(')')? + 2..];
    after.split_whitespace().nth(19).map(str::to_string)
}

/// The one request `manager_decision` relays: `decision_read` for the resource the agent named, a
/// UUID, and nothing else of the agent's.
fn decision_request(request: &Value) -> Result<Value, Error> {
    let resource = lc::text(request, "resource")?;
    if !lc::is_uuid(resource) {
        return Err("resource must be the UUID of a resource the replicas decide".into());
    }
    Ok(json!({"operation": "decision_read", "resource": resource}))
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
        // A resident that has not yet caught up with its peers since it started appends nothing
        // and says so; the same operation is retried as is once it has (the resident's catch-up rule).
        if error == "append_observation_catching_up" {
            return Err("the resident has not caught up with its peers since it started (append_observation_catching_up): nothing was appended; retry the same operation".into());
        }
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
    lc::fault("manager-before-relay")?;
    if identity(door.pid, &door.container_id).as_deref() != Some(door.started.as_str()) {
        return Err(format!("manager operation refused: the process of container {} changed between its inspection and the relay (PID {} gone, reused, or moved); nothing was sent", door.container_id, door.pid).into());
    }
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
    // The same incarnation must still be there after the relay, or the answer came from something
    // else and is not trusted; a journaled observation then fails and its retry replays the same
    // operation ID against the resident, which appends nothing twice.
    if identity(door.pid, &door.container_id).as_deref() != Some(door.started.as_str()) {
        return Err(format!("manager operation refused: the process of container {} changed during the relay; its answer is not trusted", door.container_id).into());
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

pub(crate) const CONTROL_SOCKET_PATH: &str = CONTROL_SOCKET;

#[cfg(test)]
mod tests {
    use super::*;

    /// Proves: a replica's host state is three mounts derived from its name alone -- the vote directory
    /// read-write, the evidence directory and the host's machine-id read-only, at fixed paths -- made
    /// private at the first create and found again at the next; a name that is not a token, and a
    /// directory replaced by a symlink, are refused.
    #[test]
    fn a_replicas_host_state_is_three_fixed_mounts() {
        let root = std::env::temp_dir().join(format!("podmesh-host-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let (args, label) = host_state_mounts_in(&root, "lab-a").unwrap();
        let votes = root.join("lab-a/votes");
        assert_eq!(args, vec![
            "--mount".to_string(), format!("type=bind,src={},dst=/run/podmesh-host/votes", votes.display()),
            "--mount".to_string(), format!("type=bind,src={},dst=/run/podmesh-host/evidence,ro=true", root.join("lab-a/evidence").display()),
            "--mount".to_string(), "type=bind,src=/etc/machine-id,dst=/run/podmesh-host/machine-id,ro=true".to_string(),
        ]);
        assert_eq!(label, "io.podmesh.manager-host-state=lab-a");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&votes).unwrap().permissions().mode() & 0o777, 0o700);
        std::fs::write(votes.join("replica-a.ledger"), "kept").unwrap();
        assert_eq!(host_state_mounts_in(&root, "lab-a").unwrap().0, args, "the same name, the same mounts");
        assert_eq!(std::fs::read_to_string(votes.join("replica-a.ledger")).unwrap(), "kept", "nothing is removed");
        std::fs::create_dir_all(root.join("elsewhere")).unwrap();
        std::fs::remove_dir_all(root.join("lab-a/evidence")).unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere"), root.join("lab-a/evidence")).unwrap();
        assert!(host_state_mounts_in(&root, "lab-a").is_err(), "a symlinked directory");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Proves: the door relays exactly one `decision_read` naming the resource the agent named, a UUID,
    /// and nothing else of the request; any other resource is refused before a connection is made.
    #[test]
    fn a_decision_read_relays_one_request_for_one_resource() {
        let r = "91eeb6bf-5489-405b-b77a-53105b0aff7a";
        let relayed = decision_request(&json!({"operation": "manager_decision", "universe_uuid": r, "resource": r,
                                               "authorization_ref": "x", "extra": {"operation": "vote_sign"}}))
        .unwrap();
        assert_eq!(relayed, json!({"operation": "decision_read", "resource": r}));
        for bad in [json!({}), json!({"resource": ""}), json!({"resource": "vote_sign"}), json!({"resource": format!("{r}x")})] {
            assert!(decision_request(&bad).is_err(), "{bad}");
        }
    }
}
