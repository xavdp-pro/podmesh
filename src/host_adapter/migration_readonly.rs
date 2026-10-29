//! Scoped host observation only. No checkpoint, repair, reservation or local APP fallback.
use super::{digest, fields, text, uuid, Error, Policy, ACTION_LABEL, SCOPE_LABEL};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::Read,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};

const MAX_RECORD: usize = 65536;
const MAX_FACTS: usize = 65536;
const MAX_PROCESSES: usize = 64;
const MAX_CGROUP_DIRS: usize = 128;

fn read_bounded(path: &Path, maximum: usize) -> Result<Vec<u8>, Error> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let mut bytes = Vec::new();
    file.take(maximum as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        return Err("migration observation input exceeds bound".into());
    }
    Ok(bytes)
}
fn string_file(path: &Path, maximum: usize) -> Result<String, Error> {
    Ok(String::from_utf8(read_bounded(path, maximum)?)?)
}

fn validate_action(scope: &str, action: &Value) -> Result<(), Error> {
    fields(
        action,
        &[
            "kind",
            "scope",
            "uuid",
            "container_id",
            "image",
            "creation_operation",
        ],
    )?;
    if action["kind"] != "migration_observe" || text(action, "scope")? != scope {
        return Err("migration observation outside provider scope".into());
    }
    uuid(text(action, "uuid")?)?;
    digest(text(action, "container_id")?)?;
    digest(text(action, "image")?)?;
    let operation = text(action, "creation_operation")?;
    if operation.is_empty() || operation.len() > 128 {
        return Err("migration observation requires a bounded creation operation".into());
    }
    crate::lifecycle::token(operation)?;
    Ok(())
}

// Deliberately independent of owned_observation: an unpinned record refuses,
// rather than adopting a container or writing its ID back to the registry.
fn validate_owned(scope: &str, action: &Value, record: &Value, c: &Value) -> Result<(), Error> {
    validate_action(scope, action)?;
    let id = text(record, "container_id")?;
    digest(id)?;
    if id != text(action, "container_id")? || c["Id"] != id {
        return Err("migration observation container identity changed or unpinned".into());
    }
    let labels = &c["Config"]["Labels"];
    digest(text(record, "action_sha256")?)?;
    if labels[SCOPE_LABEL] != scope
        || labels["io.podmesh.universe"] != action["uuid"]
        || labels[ACTION_LABEL] != record["action_sha256"]
        || labels["io.podmesh.creation-operation"] != action["creation_operation"]
        || record["operation_id"] != action["creation_operation"]
    {
        return Err("migration observation ownership binding differs".into());
    }
    let image = digest(text(action, "image")?)?;
    if digest(text(c, "Image")?)? != image {
        return Err("migration observation image binding differs".into());
    }
    Ok(())
}

#[derive(PartialEq, Eq)]
struct Registry {
    bytes: Vec<u8>,
    identity: (u64, u64, i64, i64, i64, i64),
}
fn registry(path: &Path) -> Result<Registry, Error> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.uid() != 0 || meta.permissions().mode() & 0o777 != 0o600 {
        return Err("migration observation requires a protected pinned ownership record".into());
    }
    let identity = |m: &fs::Metadata| {
        (
            m.dev(),
            m.ino(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        )
    };
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NOATIME | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)?;
    let opened = file.metadata()?;
    if !opened.is_file()
        || opened.uid() != 0
        || opened.permissions().mode() & 0o777 != 0o600
        || identity(&meta) != identity(&opened)
    {
        return Err("migration observation registry fd differs from protected path".into());
    }
    let mut bytes = Vec::new();
    (&file)
        .take(MAX_RECORD as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_RECORD {
        return Err("migration observation registry exceeds bound".into());
    }
    let after = fs::symlink_metadata(path)?;
    if identity(&meta) != identity(&after) || identity(&opened) != identity(&file.metadata()?) {
        return Err("migration observation ownership record changed during read".into());
    }
    Ok(Registry {
        bytes,
        identity: identity(&after),
    })
}

fn unchanged(before: &Value, after: &Value) -> Result<(), Error> {
    if before != after {
        return Err(
            "migration observation incarnation changed; retry requires a fresh observation".into(),
        );
    }
    Ok(())
}

fn namespace_ids(pid: &str) -> Result<Value, Error> {
    let mut ids = json!({});
    for name in ["mnt", "pid", "net", "user", "cgroup"] {
        // PID is derived from the owned container, never supplied by the client.
        ids[name] = json!(fs::read_link(format!("/proc/{pid}/ns/{name}"))?.to_string_lossy());
    }
    Ok(ids)
}
fn process_directory(pid: u32) -> Result<File, Error> {
    Ok(OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(format!("/proc/{pid}"))?)
}
fn read_process(dir: &File, name: &str, maximum: usize) -> Result<Vec<u8>, Error> {
    let name = std::ffi::CString::new(name)?;
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err("pinned process became unavailable".into());
    }
    let file = unsafe { File::from_raw_fd(fd) };
    let mut bytes = Vec::new();
    file.take(maximum as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        return Err("pinned process facts exceed bound".into());
    }
    Ok(bytes)
}
fn process_namespace_ids(dir: &File) -> Result<Value, Error> {
    let mut ids = json!({});
    for name in ["mnt", "pid", "net", "user", "cgroup"] {
        let path = std::ffi::CString::new(format!("ns/{name}"))?;
        let mut bytes = [0u8; 128];
        let n = unsafe {
            libc::readlinkat(
                dir.as_raw_fd(),
                path.as_ptr(),
                bytes.as_mut_ptr() as *mut libc::c_char,
                bytes.len(),
            )
        };
        if n <= 0 || n as usize >= bytes.len() {
            return Err("pinned process namespace unavailable".into());
        }
        ids[name] = json!(std::str::from_utf8(&bytes[..n as usize])?);
    }
    Ok(ids)
}
fn process_from(dir: &File, pid: u32, cgroup: &str) -> Result<Value, Error> {
    let stat = String::from_utf8(read_process(dir, "stat", 8192)?)?;
    let tail = stat.rsplit_once(')').ok_or("invalid process identity")?.1;
    let start = tail
        .split_whitespace()
        .nth(19)
        .ok_or("missing process start time")?
        .parse::<u64>()?;
    let groups = String::from_utf8(read_process(dir, "cgroup", 8192)?)?;
    let observed = groups
        .lines()
        .find_map(|l| l.strip_prefix("0::"))
        .ok_or("unified process cgroup required")?;
    if observed != cgroup && !observed.starts_with(&format!("{cgroup}/")) {
        return Err("owned process left its observed container cgroup".into());
    }
    Ok(
        json!({"pid":pid,"starttime_ticks":start,"cgroup":observed,"namespaces":process_namespace_ids(dir)?}),
    )
}
fn process_identity(pid: u32, cgroup: &str) -> Result<Value, Error> {
    process_from(&process_directory(pid)?, pid, cgroup)
}
fn within(deadline: Instant) -> Result<(), Error> {
    if Instant::now() >= deadline {
        return Err("migration observation time budget exhausted".into());
    }
    Ok(())
}
fn collect_pids(root: &Path, deadline: Instant) -> Result<Vec<u32>, Error> {
    let mut pending = vec![root.to_path_buf()];
    let mut pids = BTreeSet::new();
    let mut directories = 0;
    while let Some(dir) = pending.pop() {
        directories += 1;
        if directories > MAX_CGROUP_DIRS || Instant::now() > deadline {
            return Err("migration observation cgroup traversal exceeds bound".into());
        }
        if !fs::symlink_metadata(&dir)?.is_dir() {
            return Err("migration observation cgroup changed type".into());
        }
        if string_file(&dir.join("cgroup.type"), 128)?.trim() != "domain" {
            return Err("migration observation refuses threaded or unknown cgroups".into());
        }
        for word in string_file(&dir.join("cgroup.procs"), 65536)?.split_whitespace() {
            pids.insert(word.parse::<u32>()?);
            if pids.len() > MAX_PROCESSES {
                return Err("migration observation process count exceeds bound".into());
            }
        }
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
                if pending.len() + directories > MAX_CGROUP_DIRS {
                    return Err("migration observation cgroup traversal exceeds bound".into());
                }
            }
        }
    }
    Ok(pids.into_iter().collect())
}
fn incarnation(c: &Value, deadline: Instant) -> Result<Value, Error> {
    within(deadline)?;
    if c["State"]["Running"] != true || c["State"]["Paused"] != false {
        return Err("migration observation requires a running unpaused owned container".into());
    }
    if text(&c["State"], "StartedAt")?.is_empty() {
        return Err("owned container has no observed start timestamp".into());
    }
    let pid = c["State"]["Pid"]
        .as_u64()
        .filter(|v| *v > 0 && *v <= u32::MAX as u64)
        .ok_or("owned container has no live host PID")? as u32;
    let group = text(&c["State"], "CgroupPath")?;
    if group != format!("/machine.slice/libpod-{}.scope", text(c, "Id")?)
        || Path::new(group)
            .components()
            .any(|p| !matches!(p, Component::RootDir | Component::Normal(_)))
    {
        return Err("owned container cgroup outside observed libpod scope".into());
    }
    let root = PathBuf::from(format!("/sys/fs/cgroup{group}"));
    let pids = collect_pids(&root, deadline)?;
    if !pids.contains(&pid) {
        return Err("owned container init absent from observed cgroup".into());
    }
    let processes = pids
        .into_iter()
        .map(|p| {
            within(deadline)?;
            process_identity(p, group)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(
        json!({"container_id":c["Id"],"image":c["Image"],"started_at":c["State"]["StartedAt"],
        "host_pid":pid,"cgroup":group,"processes":processes,
        "boot_id":string_file(Path::new("/proc/sys/kernel/random/boot_id"),128)?.trim()}),
    )
}

fn host_command(args: &[&str], deadline: Instant) -> Result<Value, Error> {
    within(deadline)?;
    let seconds = deadline
        .saturating_duration_since(Instant::now())
        .as_secs()
        .max(1)
        .min(10);
    let out = super::command(
        seconds,
        &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
    )?;
    if !out.status.success() {
        return Err("read-only host observation command failed".into());
    }
    Ok(serde_json::from_slice(&out.stdout)?)
}
// The closed command is bounded by the same observation deadline/output cap.
// An inspect-size failure never becomes a reassuring measured zero.
fn writable_layer_with(
    c: &Value, run: &Value, deadline: Instant,
    collect: impl FnOnce(&[&str], Instant) -> Result<Value, Error>,
) -> Result<Value, Error> {
    within(deadline)?;
    if c["State"]["Running"].as_bool()!=Some(true) || c["State"]["Paused"].as_bool()!=Some(false) {
        return Err("writable-layer observation requires a running unpaused container".into());
    }
    let id=text(c,"Id")?;digest(id)?;
    let observed=collect(&["container","inspect","--size",id],deadline)?;
    within(deadline)?;
    let rows=observed.as_array().filter(|rows|rows.len()==1).ok_or("sized container observation absent or ambiguous")?;
    let sized=&rows[0];
    for key in ["Id","Image"] {
        if sized[key]!=c[key] {return Err("sized observation container/image changed".into());}
    }
    for key in [SCOPE_LABEL,ACTION_LABEL,"io.podmesh.universe","io.podmesh.creation-operation"] {
        if sized["Config"]["Labels"][key]!=c["Config"]["Labels"][key]
            || c["Config"]["Labels"][key].as_str().filter(|v|!v.is_empty()).is_none() {
            return Err("sized observation ownership binding changed".into());
        }
    }
    for key in ["Pid","StartedAt","CgroupPath","Running","Paused"] {
        if sized["State"][key]!=c["State"][key] {return Err("sized observation process incarnation changed".into());}
    }
    if sized["State"]["Pid"]!=run["host_pid"] || sized["State"]["StartedAt"]!=run["started_at"]
        || sized["State"]["CgroupPath"]!=run["cgroup"] {
        return Err("sized observation differs from sampled process incarnation".into());
    }
    let bytes=sized["SizeRw"].as_u64().ok_or("writable-layer SizeRw missing, negative or not a u64")?;
    Ok(json!({"type":"podmesh-owned-writable-layer/1","status":"observed","source":"provider-podman-inspect-size",
        "bytes":bytes,"container_id":id,"image":c["Image"],"scope":c["Config"]["Labels"][SCOPE_LABEL],
        "universe_uuid":c["Config"]["Labels"]["io.podmesh.universe"],
        "creation_operation":c["Config"]["Labels"]["io.podmesh.creation-operation"],
        "incarnation":{"host_pid":run["host_pid"],"started_at":run["started_at"],"cgroup":run["cgroup"]},
        "limit":"point-in-time running writable size; not a future hard bound or reservation; recheck after quiescence"}))
}
fn filesystem(path: &Path) -> Result<Value, Error> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let canonical = fs::canonicalize(path)?;
    let c = CString::new(canonical.as_os_str().as_bytes())?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut stat) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if stat.f_bavail>stat.f_bfree || stat.f_bfree>stat.f_blocks {return Err("graphroot free-block counts inconsistent".into());}
    let block=u64::try_from(stat.f_frsize)?;
    if block==0 {return Err("graphroot fragment size is zero".into());}
    let available=u64::try_from(stat.f_bavail)?.checked_mul(block).ok_or("graphroot free-space overflow")?;
    Ok(json!({"path":canonical,"available_bytes":available}))
}
// An environment hint is not proof of kernel rseq registration or restorability.
fn classify_process(maps: &str, environ: &[u8]) -> Value {
    let tunables = environ
        .split(|b| *b == 0)
        .find_map(|v| v.strip_prefix(b"GLIBC_TUNABLES="));
    let zero = tunables.and_then(|v| {
        v.split(|b| *b == b':')
            .filter_map(|part| part.strip_prefix(b"glibc.pthread.rseq="))
            .last()
    }) == Some(b"0".as_slice());
    let libc = if maps.contains("/libc.so.6") {
        "glibc"
    } else if maps.contains("/ld-musl-") {
        "musl"
    } else {
        "unidentified"
    };
    json!({"libc":libc,"glibc_rseq_zero_tunable_observed":zero})
}
fn facts(c: &Value, run: &Value, deadline: Instant) -> Result<Value, Error> {
    within(deadline)?;
    let mut processes = Vec::new();
    for identity in run["processes"]
        .as_array()
        .ok_or("missing process identities")?
    {
        let pid = identity["pid"].as_u64().ok_or("invalid process identity")?;
        within(deadline)?;
        let pid = u32::try_from(pid)?;
        let dir = process_directory(pid)?;
        unchanged(identity, &process_from(&dir, pid, text(run, "cgroup")?)?)?;
        let maps = String::from_utf8(read_process(&dir, "maps", 4 * 1024 * 1024)?)?;
        let environ = read_process(&dir, "environ", 65536)?;
        unchanged(identity, &process_from(&dir, pid, text(run, "cgroup")?)?)?;
        within(deadline)?;
        let mut summary = classify_process(&maps, &environ);
        summary["identity"] = identity.clone();
        processes.push(summary);
    }
    let root = PathBuf::from(format!("/sys/fs/cgroup{}", text(run, "cgroup")?));
    let memory = string_file(&root.join("memory.current"), 128)?
        .trim()
        .parse::<u64>()?;
    let events = string_file(&root.join("cgroup.events"), 4096)?;
    let info = host_command(&["info", "--format", "json"], deadline)?;
    let graph = info["store"]["graphRoot"]
        .as_str()
        .ok_or("actual host graphroot unavailable")?;
    if !Path::new(graph).is_absolute() {
        return Err("actual host graphroot is not absolute".into());
    }
    let writable=writable_layer_with(c,run,deadline,host_command)?;
    let mut runtime = json!({});
    for path in [
        "/usr/lib/podmesh-vzcriu/criu",
        "/usr/bin/podmesh-vzcriu",
        "/opt/podmesh-vzcriu-kit/bin/criu",
    ] {
        within(deadline)?;
        runtime[path] = match hash_runtime(Path::new(path), deadline) {
            Ok(hash) => json!({"sha256":hash}),
            Err(_) => json!({"observation":"unavailable_or_refused_by_bounded_nofollow_read"}),
        };
    }
    Ok(
        json!({"provider_namespaces":namespace_ids("self")?,"container_incarnation":run,
        "processes":processes,"memory_current_bytes":memory,"frozen":events.lines().any(|l|l=="frozen 1"),
        "container_shape":{"privileged":c["HostConfig"]["Privileged"],"network_mode":c["HostConfig"]["NetworkMode"],
            "mounts_present":c["Mounts"].as_array().map(|v|!v.is_empty()),"tty":c["Config"]["Tty"]},
        "host_kernel":string_file(Path::new("/proc/sys/kernel/osrelease"),4096)?.trim(),
        "runtime_files":runtime,"runtime_check":"not executed: observation does not invoke CRIU",
        "podman_runtime":{"version":info["version"]["Version"],"oci_runtime":info["host"]["ociRuntime"]["name"],
            "oci_runtime_version":info["host"]["ociRuntime"]["version"]},
        "storage":{"graphroot":graph,"driver":info["store"]["graphDriverName"],"writable_layer_bytes":writable["bytes"],
            "writable_layer":writable,"writable_layer_status":"observed point-in-time; bounded command; not a reservation",
            "graphroot_filesystem":filesystem(Path::new(graph))?,"app_artifact_volume":null,
            "app_artifact_volume_status":"not observable through this host capability; no APP free-space claim",
            "legacy_default_graphroot":graph=="/var/lib/containers/storage"},
        "qualification":"observation only; storage/runtime and migration compatibility are not qualified"}),
    )
}

// The same bounded-read transaction is exercised with deterministic observations
// in focused tests; production supplies real host collectors, never client facts.
fn observe_owned(
    scope: &str,
    action: &Value,
    path: &Path,
    mut inspect: impl FnMut() -> Result<Value, Error>,
    mut sample: impl FnMut(&Value) -> Result<Value, Error>,
    collect: impl FnOnce(&Value, &Value) -> Result<Value, Error>,
) -> Result<Value, Error> {
    validate_action(scope, action)?;
    let id = text(action, "uuid")?;
    let initial = registry(path)?;
    let record: Value = serde_json::from_slice(&initial.bytes)?;
    let c = inspect()?;
    validate_owned(scope, action, &record, &c)?;
    let before = sample(&c)?;
    let observation = collect(&c, &before)?;
    let after_c = inspect()?;
    validate_owned(scope, action, &record, &after_c)?;
    unchanged(&before, &sample(&after_c)?)?;
    if initial != registry(path)? {
        return Err("migration observation ownership record changed".into());
    }
    let result = json!({"type":"podmesh-owned-migration-observation/1","scope":scope,
        "universe_uuid":id,"creation_operation":action["creation_operation"],"observed_at":super::epoch_seconds(),
        "facts":observation,"effects":"none: no registry repair, effect intent, suspension or artifact write"});
    if serde_json::to_vec(&result)?.len() > MAX_FACTS {
        return Err("migration observation output exceeds bound".into());
    }
    Ok(result)
}

pub(super) fn observe(policy: &Policy, action: &Value) -> Result<Value, Error> {
    validate_action(&policy.scope, action)?;
    let provider_ns = namespace_ids("self")?;
    let init_ns = namespace_ids("1")?;
    if provider_ns["cgroup"] != init_ns["cgroup"] || provider_ns["pid"] != init_ns["pid"] {
        return Err("migration observer must share visible init PID/cgroup namespaces".into());
    }
    let id = text(action, "uuid")?;
    let deadline = Instant::now() + Duration::from_secs(10);
    observe_owned(
        &policy.scope,
        action,
        &policy.record_path(id)?,
        || {
            // No lifecycle inspect fallback/repair, and no caller-selected command.
            let observed = host_command(
                &["container", "inspect", &format!("podmesh-{id}")],
                deadline,
            )?;
            let containers = observed
                .as_array()
                .ok_or("invalid owned container observation")?;
            if containers.len() != 1 {
                return Err("owned migration source absent or ambiguous".into());
            }
            Ok(containers[0].clone())
        },
        |c| incarnation(c, deadline),
        |c, run| facts(c, run, deadline),
    )
}

fn hash_runtime(path: &Path, deadline: Instant) -> Result<String, Error> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err("runtime hash requires a regular file".into());
    }
    let mut hash = blake3::Hasher::new();
    let mut total = 0;
    let mut bytes = [0u8; 65536];
    loop {
        within(deadline)?;
        let n = file.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        total += n;
        if total > 64 * 1024 * 1024 {
            return Err("runtime hash input exceeds bound".into());
        }
        hash.update(&bytes[..n]);
    }
    Ok(hash.finalize().to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Value, Value, Value) {
        let action = json!({"kind":"migration_observe","scope":"test","uuid":"00000000-0000-4000-8000-000000000001",
            "container_id":"a".repeat(64),"image":"b".repeat(64),"creation_operation":"create-owned"});
        let record = json!({"container_id":"a".repeat(64),"operation_id":"create-owned","action_sha256":"c".repeat(64)});
        let c = json!({"Id":"a".repeat(64),"Image":"b".repeat(64),"Config":{"Labels":{
            "io.podmesh.adapter-scope":"test","io.podmesh.universe":action["uuid"],
            "io.podmesh.adapter-action":"c".repeat(64),"io.podmesh.creation-operation":"create-owned"}}});
        (action, record, c)
    }
    #[test]
    fn migration_readonly_binding_never_repairs_unpinned_or_foreign_records() {
        let (a, r, c) = fixture();
        let original = serde_json::to_vec(&r).unwrap();
        assert!(validate_owned("test", &a, &r, &c).is_ok());
        for key in ["container_id", "operation_id", "action_sha256"] {
            let mut bad = r.clone();
            bad.as_object_mut().unwrap().remove(key);
            let bytes = serde_json::to_vec(&bad).unwrap();
            assert!(validate_owned("test", &a, &bad, &c).is_err());
            assert_eq!(bytes, serde_json::to_vec(&bad).unwrap());
        }
        assert_eq!(original, serde_json::to_vec(&r).unwrap());
        for (key, value) in [
            ("scope", json!("foreign")),
            ("container_id", json!("d".repeat(64))),
            ("image", json!("d".repeat(64))),
            ("creation_operation", json!("other-create")),
            ("uuid", json!("00000000-0000-4000-8000-000000000002")),
        ] {
            let mut bad = a.clone();
            bad[key] = value;
            assert!(validate_owned("test", &bad, &r, &c).is_err());
        }
        let mut replaced = c.clone();
        replaced["Id"] = json!("d".repeat(64));
        assert!(validate_owned("test", &a, &r, &replaced).is_err());
        let mut bad = c.clone();
        bad["Config"]["Labels"][SCOPE_LABEL] = json!("foreign");
        assert!(validate_owned("test", &a, &r, &bad).is_err());
    }
    #[test]
    fn migration_readonly_rejects_client_paths_pids_commands_and_unknown_fields() {
        let (a, _, _) = fixture();
        for key in ["path", "pid", "command", "cgroup", "runtime", "extra"] {
            let mut bad = a.clone();
            bad[key] = json!("untrusted");
            assert!(validate_action("test", &bad).is_err());
        }
    }
    #[test]
    fn migration_readonly_incarnation_change_fails_closed() {
        let run = json!({"container_id":"a","host_pid":42,"started_at":"run1","cgroup":"/owned",
            "processes":[{"pid":42,"starttime_ticks":100,"namespaces":{"pid":"pid:[1]"}}]});
        assert!(unchanged(&run, &run).is_ok());
        for (pointer, value) in [
            ("/host_pid", json!(43)),
            ("/started_at", json!("run2")),
            ("/cgroup", json!("/foreign")),
            ("/processes/0/starttime_ticks", json!(101)),
            ("/processes/0/namespaces/pid", json!("pid:[2]")),
        ] {
            let mut changed = run.clone();
            *changed.pointer_mut(pointer).unwrap() = value;
            assert!(unchanged(&run, &changed).is_err());
        }
    }

    #[test]
    fn migration_readonly_registry_bytes_and_inode_survive_success_and_refusal() {
        let (a, r, c) = fixture();
        let dir = std::env::temp_dir().join(format!(
            "podmesh-migration-readonly-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&dir).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(dir.clone());
        let path = dir.join("owned.json");
        fs::write(&path, serde_json::to_vec(&r).unwrap()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        if unsafe { libc::geteuid() } != 0 {
            assert!(registry(&path).is_err(), "non-root registry must refuse");
            eprintln!("root-only success/refusal registry cases require the CT root test run");
            return;
        }
        let initial = registry(&path).unwrap();
        let run = json!({"host_pid":42,"starttime_ticks":100});
        let out = observe_owned(
            "test",
            &a,
            &path,
            || Ok(c.clone()),
            |_| Ok(run.clone()),
            |_, _| Ok(json!({"synthetic_unit_test":true})),
        );
        assert!(out.is_ok());
        assert!(initial == registry(&path).unwrap());
        let mut samples = 0;
        let changed = observe_owned(
            "test",
            &a,
            &path,
            || Ok(c.clone()),
            |_| {
                samples += 1;
                Ok(json!({"host_pid":42,"starttime_ticks":samples}))
            },
            |_, _| Ok(json!({})),
        );
        assert!(changed.is_err());
        assert!(initial == registry(&path).unwrap());
        let oversized = observe_owned(
            "test",
            &a,
            &path,
            || Ok(c.clone()),
            |_| Ok(run.clone()),
            |_, _| Ok(json!({"bounded_test":"x".repeat(MAX_FACTS)})),
        );
        assert!(oversized.is_err());
        assert!(initial == registry(&path).unwrap());
        let altered = observe_owned(
            "test",
            &a,
            &path,
            || Ok(c.clone()),
            |_| Ok(run.clone()),
            |_, _| {
                let mut changed = r.clone();
                changed["operation_id"] = json!("replaced-during-observation");
                fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
                Ok(json!({}))
            },
        );
        assert!(altered.is_err());
        fs::write(&path, serde_json::to_vec(&r).unwrap()).unwrap();
        let mut unpinned = r.clone();
        unpinned.as_object_mut().unwrap().remove("container_id");
        fs::write(&path, serde_json::to_vec(&unpinned).unwrap()).unwrap();
        let initial = registry(&path).unwrap();
        assert!(observe_owned(
            "test",
            &a,
            &path,
            || Ok(c.clone()),
            |_| panic!("must refuse before host sampling"),
            |_, _| panic!("must not collect")
        )
        .is_err());
        assert!(initial == registry(&path).unwrap());
        fs::remove_file(&path).unwrap();
        assert!(observe_owned(
            "test",
            &a,
            &path,
            || panic!("missing ownership must refuse first"),
            |_| panic!("no sample"),
            |_, _| panic!("no facts")
        )
        .is_err());
        assert!(!path.exists());
    }

    #[test]
    fn migration_readonly_dispatch_rejects_effect_intent_without_host_access() {
        let (a, _, _) = fixture();
        let policy = Policy {
            scope: "test".into(),
            uid: 1102,
            gid: 1102,
            images: vec![],
            nested: vec![],
            contained: vec![],
            operations: vec![],
            max_memory: 0,
            max_cpus: 0.0,
            max_universes: 1,
            state: PathBuf::from("/not-used"),
        };
        let request =
            json!({"protocol":super::super::PROTOCOL,"action":a,"intent":{"operation":"start"}});
        let err = super::super::execute(&policy, &request)
            .unwrap_err()
            .to_string();
        assert!(err.contains("refuses an effect intent"));
    }
    #[test]
    fn migration_readonly_pins_proc_directory_and_refuses_foreign_cgroup() {
        let pid = std::process::id();
        let dir = process_directory(pid).unwrap();
        let groups = String::from_utf8(read_process(&dir, "cgroup", 8192).unwrap()).unwrap();
        let group = groups
            .lines()
            .find_map(|l| l.strip_prefix("0::"))
            .expect("Linux unified cgroup test fixture");
        let first = process_from(&dir, pid, group).unwrap();
        assert_eq!(first, process_from(&dir, pid, group).unwrap());
        assert!(process_from(&dir, pid, "/definitely-not-owned").is_err());
        assert_eq!(first["namespaces"], namespace_ids("self").unwrap());
        // All process metadata reads stay on this fd, not a reopened numeric PID.
        assert!(read_process(&dir, "stat", 1).is_err());
    }

    #[test]
    fn migration_readonly_process_summary_never_contains_environment_or_maps() {
        let env = b"PASSWORD=do-not-return\0GLIBC_TUNABLES=glibc.pthread.rseq=0\0";
        let summary = classify_process("/private/path/libc.so.6", env);
        let encoded = serde_json::to_string(&summary).unwrap();
        assert_eq!(summary["libc"], "glibc");
        assert_eq!(summary["glibc_rseq_zero_tunable_observed"], true);
        for secret in [
            "PASSWORD",
            "do-not-return",
            "private/path",
            "GLIBC_TUNABLES",
        ] {
            assert!(!encoded.contains(secret));
        }
        assert_eq!(
            classify_process("/libc.so.6", b"GLIBC_TUNABLES=glibc.pthread.rseq=00\0")
                ["glibc_rseq_zero_tunable_observed"],
            false
        );
        assert_eq!(
            classify_process(
                "/libc.so.6",
                b"GLIBC_TUNABLES=glibc.pthread.rseq=0:glibc.pthread.rseq=1\0"
            )["glibc_rseq_zero_tunable_observed"],
            false
        );
    }
    fn space_fixture()->(Value,Value) {
        let (_,_,mut c)=fixture();
        c["State"]=json!({"Pid":42,"StartedAt":"one-run","CgroupPath":format!("/machine.slice/libpod-{}.scope","a".repeat(64)),"Running":true,"Paused":false});
        let run=json!({"host_pid":42,"started_at":"one-run","cgroup":c["State"]["CgroupPath"]});
        (c,run)
    }
    #[test]
    fn migration_space_size_zero_is_observed_but_invalid_or_missing_is_not_zero() {
        let (c,run)=space_fixture();
        for bytes in [0u64,1234] {
            let mut sized=c.clone();sized["SizeRw"]=json!(bytes);
            let result=writable_layer_with(&c,&run,Instant::now()+Duration::from_secs(5),|args,_|{
                let id="a".repeat(64);assert_eq!(args,["container","inspect","--size",id.as_str()]);
                Ok(json!([sized]))
            }).unwrap();
            assert_eq!(result["bytes"],bytes);assert_eq!(result["status"],"observed");
        }
        for value in [Value::Null,json!(-1),json!("0"),json!(1.5),serde_json::from_str("18446744073709551616").unwrap()] {
            let mut sized=c.clone();sized["SizeRw"]=value;
            assert!(writable_layer_with(&c,&run,Instant::now()+Duration::from_secs(5),|_,_|Ok(json!([sized]))).is_err());
        }
        assert!(writable_layer_with(&c,&run,Instant::now()+Duration::from_secs(5),|_,_|Ok(json!([c.clone()]))).is_err());
    }
    #[test]
    fn migration_space_size_identity_replacement_and_timeout_fail_closed() {
        let (c,run)=space_fixture();
        for pointer in ["/Id","/Image","/Config/Labels/io.podmesh.adapter-scope","/Config/Labels/io.podmesh.creation-operation","/Config/Labels/io.podmesh.universe","/Config/Labels/io.podmesh.adapter-action",
            "/State/Pid","/State/StartedAt","/State/CgroupPath","/State/Running","/State/Paused"] {
            let mut sized=c.clone();sized["SizeRw"]=json!(0);*sized.pointer_mut(pointer).unwrap()=json!("replaced");
            assert!(writable_layer_with(&c,&run,Instant::now()+Duration::from_secs(5),|_,_|Ok(json!([sized]))).is_err());
        }
        let mut mismatch=run.clone();mismatch["host_pid"]=json!(999);
        let mut sized=c.clone();sized["SizeRw"]=json!(0);
        assert!(writable_layer_with(&c,&mismatch,Instant::now()+Duration::from_secs(5),|_,_|Ok(json!([sized]))).is_err());
        assert!(writable_layer_with(&c,&run,Instant::now()+Duration::from_secs(5),|_,_|Err("command timeout".into())).is_err());
        assert!(writable_layer_with(&c,&run,Instant::now()-Duration::from_millis(1),|_,_|panic!("expired budget must not start tool")).is_err());
        assert!(writable_layer_with(&c,&run,Instant::now()+Duration::from_secs(5),|_,_|Ok(json!([]))).is_err());
    }

}
