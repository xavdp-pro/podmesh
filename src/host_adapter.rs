//! Typed host capabilities. The application journal remains inside its private unit.
//! See docs/HOST-ADAPTER-CONTRACT.md; isolated flat and explicitly granted nested effects.
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{MetadataExt, PermissionsExt},
            net::{UnixListener, UnixStream},
            process::ExitStatusExt,
        },
    },
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Output, Stdio},
    thread,
    time::Duration,
};
type Error = Box<dyn std::error::Error>;
pub const SOCKET_ENV: &str = "PODMESH_HOST_ADAPTER_SOCKET";
const PROTOCOL: &str = "podmesh-host-capability/1";
const SCOPE_LABEL: &str = "io.podmesh.adapter-scope";
const ACTION_LABEL: &str = "io.podmesh.adapter-action";
const MAX_REQUEST: u64 = 65536;
const MAX_OUTPUT: usize = 4 * 1024 * 1024;
thread_local! { static INTENT: RefCell<Option<Value>> = const { RefCell::new(None) }; }

pub(crate) fn configured() -> bool {
    std::env::var_os(SOCKET_ENV).is_some()
}
/// Only call after the application's pending operation/attempt transaction committed.
pub(crate) fn with_intent<T>(request: &Value, run: impl FnOnce() -> T) -> T {
    struct Restore(Option<Value>);
    impl Drop for Restore {
        fn drop(&mut self) {
            INTENT.with(|v| {
                v.replace(self.0.take());
            });
        }
    }
    let previous = INTENT.with(|v| v.replace(Some(request.clone())));
    let _restore = Restore(previous);
    run()
}
fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, Error> {
    value[key]
        .as_str()
        .ok_or_else(|| format!("missing typed field {key}").into())
}
fn fields(value: &Value, allowed: &[&str]) -> Result<(), Error> {
    let object = value
        .as_object()
        .ok_or("typed capability must be an object")?;
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err("unknown typed capability field".into());
    }
    Ok(())
}
fn uuid(value: &str) -> Result<(), Error> {
    if crate::lifecycle::is_uuid(value) {
        Ok(())
    } else {
        Err("invalid universe UUID".into())
    }
}
fn named(value: &str) -> Result<&str, Error> {
    let id = value
        .strip_prefix("podmesh-")
        .ok_or("host capability targets only PodMesh universe names")?;
    uuid(id)?;
    Ok(id)
}
fn digest(value: &str) -> Result<&str, Error> {
    let bare = value.strip_prefix("sha256:").unwrap_or(value);
    if bare.len() != 64 || !bare.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("host image must be a full local digest".into());
    }
    Ok(bare)
}
fn tokens(value: &Value) -> Result<Vec<String>, Error> {
    let items = value.as_array().ok_or("command must be a vector")?;
    if items.len() > 128 {
        return Err("command vector too large".into());
    }
    items
        .iter()
        .map(|v| {
            let s = v.as_str().ok_or("command token must be text")?;
            if s.len() > 4096 || s.contains('\0') {
                return Err("invalid command token".into());
            }
            Ok(s.to_string())
        })
        .collect()
}

/// Convert existing lifecycle-generated arguments into a typed message. Flags never cross IPC.
fn typed(args: &[&str]) -> Result<Value, Error> {
    Ok(match args {
        ["ps", "--all", "--format", "json"] => json!({"kind":"inventory"}),
        ["images", "--all", "--format", "json"] => json!({"kind":"images"}),
        ["container", "inspect", name] => json!({"kind":"inspect", "uuid":named(name)?}),
        [action @ ("start" | "pause" | "unpause" | "rm"), name] => {
            json!({"kind":action,"uuid":named(name)?})
        }
        ["stop", "--time", seconds, name] => {
            json!({"kind":"stop","uuid":named(name)?,"timeout_seconds":seconds.parse::<u64>()?})
        }
        ["kill", "--signal", signal, name] => {
            json!({"kind":"signal","uuid":named(name)?,"signal":signal})
        }
        ["image", "rm", targets @ ..] if !targets.is_empty() => {
            json!({"kind":"remove_snapshots","targets":targets})
        }
        ["create", rest @ ..] => {
            let mut i = 0;
            let mut name = None;
            let mut labels = serde_json::Map::new();
            let mut pull = false;
            let mut isolated = false;
            let mut privileged = false;
            while i < rest.len() {
                match rest[i] {
                    "--pull=never" => { pull=true; i+=1; },
                    "--network=none" => { isolated=true; i+=1; },
                    "--privileged" if !privileged => { privileged=true; i+=1; },
                    "--name" if i+1<rest.len() => { if name.is_some() { return Err("duplicate container name".into()) } name=Some(named(rest[i+1])?); i+=2; },
                    "--label" if i+1<rest.len() => { let (key,value)=rest[i+1].split_once('=').ok_or("invalid label")?; if labels.insert(key.to_string(),json!(value)).is_some() { return Err("duplicate label".into()) } i+=2; },
                    flag if flag.starts_with('-') => return Err("host adapter create refuses unported flags, nested privileges, secrets and host mounts".into()),
                    _ => break,
                }
            }
            if !pull || !isolated || i >= rest.len() {
                return Err("host adapter create requires local image and isolated network".into());
            }
            if privileged && labels.get("io.podmesh.universe-profile") != Some(&json!("nested")) {
                return Err("nested privilege must carry its typed universe profile".into());
            }
            let image = digest(rest[i])?;
            json!({"kind":"create","uuid":name.ok_or("missing container name")?,"image":image,"command":&rest[i+1..],"labels":labels,"nested":privileged})
        }
        ["update", rest @ ..] if !rest.is_empty() => {
            let id = named(rest[rest.len() - 1])?;
            let mut i = 0;
            let mut memory = None;
            let mut swap = None;
            let mut cpus = None;
            while i + 1 < rest.len() - 1 {
                match rest[i] {
                    "--memory" if memory.is_none() => memory = Some(rest[i + 1].parse::<u64>()?),
                    "--memory-swap" if swap.is_none() => swap = Some(rest[i + 1].parse::<u64>()?),
                    "--cpus" if cpus.is_none() => cpus = Some(rest[i + 1].parse::<f64>()?),
                    _ => return Err("unsupported resource flag".into()),
                };
                i += 2;
            }
            if i != rest.len() - 1 || memory != swap {
                return Err("resource arguments do not bind memory and swap".into());
            }
            json!({"kind":"resources","uuid":id,"memory_bytes":memory,"cpus":cpus})
        }
        ["commit", "--pause=false", "--change", a, "--change", b, "--change", c, source, reference] =>
        {
            let mut labels = serde_json::Map::new();
            for change in [a, b, c] {
                let (key, value) = change
                    .strip_prefix("LABEL ")
                    .ok_or("only snapshot labels may change")?
                    .split_once('=')
                    .ok_or("invalid snapshot label")?;
                if labels.insert(key.to_string(), json!(value)).is_some() {
                    return Err("duplicate snapshot label".into());
                }
            }
            json!({"kind":"snapshot","source_uuid":named(source)?,"reference":reference,"labels":labels})
        }
        _ => return Err("host capability is not ported; no local Podman fallback".into()),
    })
}
pub(crate) fn run_podman(args: &[&str]) -> Result<Output, Error> {
    let action = typed(args)?;
    let intent = INTENT.with(|v| v.borrow().clone());
    let response = call(&json!({"protocol":PROTOCOL,"action":action,"intent":intent}))?;
    let code = response["exit_code"]
        .as_i64()
        .filter(|n| (0..=255).contains(n))
        .ok_or("invalid host command status")? as i32;
    Ok(Output {
        status: ExitStatus::from_raw(code << 8),
        stdout: text(&response, "stdout")?.as_bytes().to_vec(),
        stderr: text(&response, "stderr")?.as_bytes().to_vec(),
    })
}
pub(crate) fn inspect(name: &str) -> Result<Option<Value>, Error> {
    let out = run_podman(&["container", "inspect", name])?;
    if !out.status.success() {
        return Err("host universe inspection failed".into());
    }
    let containers: Vec<Value> = serde_json::from_slice(&out.stdout)?;
    if containers.len() > 1 {
        return Err("host inspection returned multiple universe identities".into());
    }
    Ok(containers.into_iter().next())
}
pub(crate) fn fact(kind: &str, id: Option<&str>) -> Result<Value, Error> {
    let action = if let Some(id) = id {
        json!({"kind":kind,"uuid":id})
    } else {
        json!({"kind":kind})
    };
    call(&json!({"protocol":PROTOCOL,"action":action,"intent":null}))
}
/// One host namespace observation, validated before application boot decisions.
pub(crate) struct HostIdentity {
    pub machine: String,
    pub boot: String,
    pub booted: Option<i64>,
    pub clock: Option<bool>,
}
impl HostIdentity {
    fn decode(value: &Value) -> Result<Self, Error> {
        fields(
            value,
            &["machine_id", "boot_id", "booted_at", "clock_synchronized"],
        )?;
        let machine = text(value, "machine_id")?;
        if machine.len() != 32 || !machine.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err("invalid host machine identity".into());
        }
        let boot = text(value, "boot_id")?;
        uuid(boot)?;
        let booted = match &value["booted_at"] {
            Value::Null => None,
            v => Some(
                v.as_i64()
                    .filter(|n| *n > 0)
                    .ok_or("invalid host boot time")?,
            ),
        };
        let clock = match &value["clock_synchronized"] {
            Value::Null => None,
            v => Some(v.as_bool().ok_or("invalid host clock observation")?),
        };
        Ok(Self {
            machine: machine.into(),
            boot: boot.into(),
            booted,
            clock,
        })
    }
}
pub(crate) fn host_identity() -> Result<HostIdentity, Error> {
    HostIdentity::decode(&fact("host_identity", None)?)
}
fn host_identity_local() -> Result<Value, Error> {
    let clock = Command::new("/usr/bin/timeout")
        .args([
            "--kill-after=1",
            "5",
            "/usr/bin/timedatectl",
            "show",
            "-p",
            "NTPSynchronized",
            "--value",
        ])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| match String::from_utf8_lossy(&out.stdout).trim() {
            "yes" => Some(true),
            "no" => Some(false),
            _ => None,
        });
    let booted = fs::read_to_string("/proc/stat")?
        .lines()
        .find_map(|line| line.strip_prefix("btime "))
        .and_then(|n| n.trim().parse::<i64>().ok())
        .filter(|n| *n > 0);
    let facts = json!({"machine_id":fs::read_to_string("/etc/machine-id")?.trim(), "boot_id":fs::read_to_string("/proc/sys/kernel/random/boot_id")?.trim(), "booted_at":booted, "clock_synchronized":clock});
    HostIdentity::decode(&facts)?;
    Ok(facts)
}
fn call(request: &Value) -> Result<Value, Error> {
    let socket = std::env::var_os(SOCKET_ENV).ok_or("host adapter socket is not configured")?;
    let mut stream = UnixStream::connect(socket)?;
    if peer_uid(&stream)? != 0 {
        return Err("host capability provider peer is not root".into());
    }
    stream.set_read_timeout(Some(Duration::from_secs(340)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let bytes = serde_json::to_vec(request)?;
    if bytes.len() as u64 > MAX_REQUEST {
        return Err("host request too large".into());
    }
    stream.write_all(&bytes)?;
    stream.write_all(b"\n")?;
    let mut reader = BufReader::new(stream).take((2 * MAX_OUTPUT + 65536) as u64);
    let mut bytes = Vec::new();
    reader.read_until(b'\n', &mut bytes)?;
    if !bytes.ends_with(b"\n") {
        return Err("incomplete or oversized host response".into());
    }
    let response: Value = serde_json::from_slice(&bytes)?;
    if response["ok"] != true {
        return Err(response["error"]
            .as_str()
            .unwrap_or("host capability refused")
            .to_string()
            .into());
    }
    Ok(response["data"].clone())
}

fn host_capacity() -> Result<Value, Error> {
    let kib: u64 = fs::read_to_string("/proc/meminfo")?
        .lines()
        .find_map(|l| l.strip_prefix("MemTotal:"))
        .and_then(|s| s.split_whitespace().next())
        .ok_or("host memory unavailable")?
        .parse()?;
    Ok(
        json!({"memory_bytes":kib.checked_mul(1024).ok_or("host memory overflow")?,"cpus":thread::available_parallelism()?.get()}),
    )
}
fn protected_ancestors(path: &Path) -> Result<(), Error> {
    for ancestor in path.ancestors().skip(1) {
        let metadata = fs::symlink_metadata(ancestor)?;
        let mode = metadata.permissions().mode();
        if !metadata.is_dir() || metadata.uid() != 0 || (mode & 0o022 != 0 && mode & 0o1000 == 0) {
            return Err(
                "host capability paths require protected root-owned parent directories".into(),
            );
        }
    }
    Ok(())
}
struct Policy {
    scope: String,
    uid: u32,
    gid: u32,
    images: Vec<String>,
    nested: Vec<(String, String)>,
    operations: Vec<String>,
    max_memory: u64,
    max_cpus: f64,
    max_universes: u64,
    state: PathBuf,
}
impl Policy {
    fn load(path: &Path) -> Result<Self, Error> {
        if !path.is_absolute() {
            return Err("host policy path must be absolute".into());
        }
        protected_ancestors(path)?;
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_file()
            || metadata.uid() != 0
            || metadata.permissions().mode() & 0o777 != 0o600
        {
            return Err("host policy must be a regular root-owned0600 file".into());
        }
        let value: Value = serde_json::from_slice(&fs::read(path)?)?;
        fields(
            &value,
            &[
                "scope",
                "application_uid",
                "application_gid",
                "allowed_images",
                "allowed_operations",
                "allowed_nested",
                "max_memory_bytes",
                "max_cpus",
                "max_universes",
                "state_dir",
            ],
        )?;
        let scope = text(&value, "scope")?.to_string();
        if scope.is_empty() {
            return Err("host scope must not be empty".into());
        }
        crate::lifecycle::token(&scope)?;
        let uid = u32::try_from(
            value["application_uid"]
                .as_u64()
                .ok_or("application UID required")?,
        )?;
        let gid = u32::try_from(
            value["application_gid"]
                .as_u64()
                .ok_or("application GID required")?,
        )?;
        if uid <= 1000 || gid <= 1000 {
            return Err(
                "host policy requires distinct non-system application UID/GID above1000".into(),
            );
        }
        let images = tokens(&value["allowed_images"])?;
        for image in &images {
            digest(image)?;
        }
        let mut nested = Vec::new();
        if let Some(grants) = value.get("allowed_nested") {
            for grant in grants.as_array().ok_or("nested grants must be an array")? {
                fields(
                    grant,
                    &["universe_uuid", "image", "mounts", "namespaces", "devices"],
                )?;
                if grant["namespaces"] != "private-pid-ipc-uts-network-none"
                    || grant["devices"] != "privileged-host-device-access"
                {
                    return Err("nested grant must explicitly authorize the fixed Rule11 namespace/device envelope".into());
                }
                let target = text(grant, "universe_uuid")?;
                uuid(target)?;
                let image = digest(text(grant, "image")?)?;
                if grant["mounts"] != json!([]) {
                    return Err("nested grant requires an exact empty host mount envelope in this increment".into());
                }
                if nested.iter().any(|(id, _)| id == target) || nested.len() >= 1024 {
                    return Err("duplicate or oversized nested identity grant".into());
                }
                if !images
                    .iter()
                    .any(|allowed| digest(allowed).ok() == Some(image))
                {
                    return Err("nested grant image must be explicitly allowed locally".into());
                }
                nested.push((target.to_string(), image.to_string()));
            }
        }
        let operations = tokens(&value["allowed_operations"])?;
        if operations.is_empty()
            || operations.iter().any(|v| {
                ![
                    "create",
                    "delete",
                    "clone",
                    "start",
                    "stop",
                    "pause",
                    "resume",
                    "resources",
                ]
                .contains(&v.as_str())
            })
        {
            return Err("host operation policy must name bounded lifecycle operations".into());
        }
        let max_memory = value["max_memory_bytes"]
            .as_u64()
            .filter(|n| *n >= 32 * 1024 * 1024)
            .ok_or("memory ceiling required")?;
        let max_cpus = value["max_cpus"]
            .as_f64()
            .filter(|n| n.is_finite() && *n >= 0.1)
            .ok_or("CPU ceiling required")?;
        let capacity = host_capacity()?;
        if max_memory
            > capacity["memory_bytes"]
                .as_u64()
                .ok_or("host memory capacity missing")?
            || max_cpus
                > capacity["cpus"]
                    .as_f64()
                    .ok_or("host CPU capacity missing")?
        {
            return Err("host policy ceilings exceed actual host capacity".into());
        }
        let max_universes = value["max_universes"]
            .as_u64()
            .filter(|v| (1..=1024).contains(v))
            .ok_or("finite universe budget required")?;
        let state = PathBuf::from(text(&value, "state_dir")?);
        if !state.is_absolute() {
            return Err("adapter state must name an absolute private path".into());
        }
        protected_ancestors(&state)?;
        let metadata = fs::symlink_metadata(&state)?;
        if !metadata.is_dir()
            || metadata.uid() != 0
            || metadata.permissions().mode() & 0o777 != 0o700
        {
            return Err("adapter state must be an existing root-owned0700 directory".into());
        }
        Ok(Self {
            scope,
            uid,
            gid,
            images,
            nested,
            operations,
            max_memory,
            max_cpus,
            max_universes,
            state,
        })
    }
    fn require_nested(&self, target: &str, image: &str) -> Result<(), Error> {
        if self
            .nested
            .iter()
            .any(|(uuid, allowed)| uuid == target && allowed == image)
        {
            Ok(())
        } else {
            Err("nested image/UUID/mount envelope is not granted by host policy".into())
        }
    }
    fn bind_intent(&self, intent: &Value) -> Result<(), Error> {
        use sha2::{Digest, Sha256};
        let id = text(intent, "operation_id")?;
        if id.is_empty() {
            return Err("operation ID must not be empty".into());
        }
        crate::lifecycle::token(id)?;
        let binding = format!("{:x}", Sha256::digest(serde_json::to_vec(intent)?));
        let path = self.state.join(format!("intent-{id}.json"));
        if path.exists() {
            let saved: Value = serde_json::from_slice(&fs::read(path)?)?;
            if saved["request_sha256"] != binding {
                return Err("operation ID already bound to another adapter intent".into());
            }
            return Ok(());
        }
        let count = fs::read_dir(&self.state)?
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with("intent-"))
            .count();
        if count >= 10000 {
            return Err("adapter intent history budget exhausted; history is retained".into());
        }
        let tmp = self
            .state
            .join(format!(".intent-{id}.{}.tmp", std::process::id()));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        let result = (|| -> Result<(), Error> {
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
            file.write_all(&serde_json::to_vec(&json!({"request_sha256":binding}))?)?;
            file.sync_all()?;
            fs::rename(&tmp, path)?;
            fs::File::open(&self.state)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(tmp);
        }
        result
    }
    fn record_path(&self, id: &str) -> Result<PathBuf, Error> {
        uuid(id)?;
        Ok(self.state.join(format!("{id}.json")))
    }
    fn write_record(&self, id: &str, value: &Value) -> Result<(), Error> {
        let path = self.record_path(id)?;
        let tmp = self.state.join(format!(".{id}.{}.tmp", std::process::id()));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        let result = (|| -> Result<(), Error> {
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
            file.write_all(&serde_json::to_vec(value)?)?;
            file.sync_all()?;
            fs::rename(&tmp, &path)?;
            fs::File::open(&self.state)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(tmp);
        }
        result
    }
    fn inspect(&self, id: &str) -> Result<Option<Value>, Error> {
        uuid(id)?;
        let out = command(
            30,
            &[
                "container".into(),
                "inspect".into(),
                format!("podmesh-{id}"),
            ],
        )?;
        if !out.status.success() {
            let all = command(
                30,
                &[
                    "ps".into(),
                    "--all".into(),
                    "--format".into(),
                    "json".into(),
                ],
            )?;
            if !all.status.success() {
                return Err("host inventory unavailable".into());
            }
            let all: Value = serde_json::from_slice(&all.stdout)?;
            if all
                .as_array()
                .ok_or("invalid host inventory")?
                .iter()
                .any(|c| {
                    c["Names"]
                        .as_array()
                        .is_some_and(|n| n.iter().any(|n| n == &format!("podmesh-{id}")))
                })
            {
                return Err("host container inspection failed".into());
            }
            return Ok(None);
        }
        Ok(serde_json::from_slice::<Value>(&out.stdout)?
            .as_array()
            .and_then(|a| a.first())
            .cloned())
    }
    fn owned(&self, id: &str) -> Result<Value, Error> {
        let c = self.inspect(id)?.ok_or("owned universe absent")?;
        let mut record: Value = serde_json::from_slice(&fs::read(self.record_path(id)?)?)?;
        let labels = &c["Config"]["Labels"];
        if labels[SCOPE_LABEL] != self.scope
            || labels[ACTION_LABEL] != record["action_sha256"]
            || labels["io.podmesh.creation-operation"] != record["operation_id"]
        {
            return Err("container identity outside adapter ownership".into());
        }
        let actual = text(&c, "Id")?;
        if let Some(saved) = record["container_id"].as_str() {
            if saved != actual {
                return Err("adapter-owned container was replaced".into());
            }
        } else {
            record["container_id"] = json!(actual);
            self.write_record(id, &record)?;
        }
        Ok(c)
    }
    fn images(&self) -> Result<Vec<Value>, Error> {
        let out = command(
            30,
            &[
                "images".into(),
                "--all".into(),
                "--format".into(),
                "json".into(),
            ],
        )?;
        if !out.status.success() {
            return Err("host image inventory unavailable".into());
        }
        let images: Vec<Value> = serde_json::from_slice(&out.stdout)?;
        Ok(images
            .into_iter()
            .filter(|i| {
                self.images.iter().any(|allowed| {
                    digest(allowed).ok() == i["Id"].as_str().and_then(|id| digest(id).ok())
                }) || i["Labels"][SCOPE_LABEL] == self.scope
            })
            .collect())
    }
}
fn command(seconds: u64, args: &[String]) -> Result<Output, Error> {
    let mut child = Command::new("/usr/bin/timeout")
        .env_remove("INVOCATION_ID")
        .args([
            "--signal=TERM",
            "--kill-after=5",
            &seconds.to_string(),
            "/usr/bin/podman",
        ])
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    fn drain(mut source: impl Read) -> (Vec<u8>, bool) {
        let mut out = Vec::new();
        let mut bytes = [0; 8192];
        let mut incomplete = false;
        loop {
            match source.read(&mut bytes) {
                Ok(0) => break,
                Err(_) => {
                    incomplete = true;
                    break;
                }
                Ok(n) => {
                    let remaining = MAX_OUTPUT.saturating_sub(out.len());
                    if n > remaining {
                        incomplete = true;
                    }
                    out.extend_from_slice(&bytes[..n.min(remaining)]);
                }
            }
        }
        (out, incomplete)
    }
    let stdout = child.stdout.take().ok_or("missing host stdout")?;
    let stderr = child.stderr.take().ok_or("missing host stderr")?;
    let a = thread::spawn(move || drain(stdout));
    let b = thread::spawn(move || drain(stderr));
    let status = child.wait()?;
    let (stdout, bad_stdout) = a.join().map_err(|_| "host stdout reader failed")?;
    let (stderr, bad_stderr) = b.join().map_err(|_| "host stderr reader failed")?;
    if bad_stdout || bad_stderr {
        return Err("host command output exceeded bounds or could not be fully observed".into());
    }
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}
fn output(out: Output) -> Value {
    json!({"exit_code":out.status.code().unwrap_or(137),"stdout":String::from_utf8_lossy(&out.stdout),"stderr":String::from_utf8_lossy(&out.stderr)})
}
fn peer_uid(stream: &UnixStream) -> Result<u32, Error> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut credentials as *mut _ as *mut libc::c_void,
            &mut length,
        )
    };
    if result != 0 || length as usize != std::mem::size_of::<libc::ucred>() {
        return Err("cannot verify host capability peer".into());
    }
    Ok(credentials.uid)
}

fn validate_intent<'a>(action: &Value, intent: &'a Value) -> Result<(&'a str, &'a str), Error> {
    let operation = text(intent, "operation")?;
    let id = text(intent, "operation_id")?;
    if id.is_empty() {
        return Err("operation ID must not be empty".into());
    }
    crate::lifecycle::token(id)?;
    let target = text(intent, "universe_uuid")?;
    uuid(target)?;
    if text(intent, "authorization_ref")?.is_empty() {
        return Err("host effect needs a mandate reference".into());
    }
    let expected = match text(action, "kind")? {
        "create" => matches!(operation, "create" | "clone"),
        "start" => operation == "start",
        "pause" => operation == "pause",
        "unpause" => operation == "resume",
        "rm" | "remove_snapshots" => operation == "delete" || operation == "clone",
        "stop" | "signal" => operation == "stop",
        "resources" => operation == "resources",
        "snapshot" => operation == "clone",
        _ => false,
    };
    if !expected {
        return Err("typed effect does not belong to the committed operation".into());
    }
    if let Some(actual) = action["uuid"].as_str() {
        if actual != target {
            return Err("typed effect targets a different universe".into());
        }
    }
    if action["kind"] == "snapshot" && text(action, "source_uuid")? != text(intent, "source_uuid")?
    {
        return Err("clone source is outside operation binding".into());
    }
    Ok((id, target))
}
fn verify_created_envelope(container: &Value, nested: bool) -> Result<(), Error> {
    if container["HostConfig"]["Privileged"].as_bool() != Some(nested)
        || container["HostConfig"]["NetworkMode"] != "none"
        || !container["Mounts"].as_array().is_some_and(Vec::is_empty)
    {
        return Err("observed universe differs from isolated privilege/mount envelope".into());
    }
    for field in ["PidMode", "IpcMode", "UTSMode"] {
        if !matches!(
            container["HostConfig"][field].as_str(),
            Some("" | "private")
        ) {
            return Err("observed universe does not carry the private namespace envelope".into());
        }
    }
    Ok(())
}
fn execute(policy: &Policy, request: &Value) -> Result<Value, Error> {
    fields(request, &["protocol", "action", "intent"])?;
    if request["protocol"] != PROTOCOL {
        return Err("unknown host protocol".into());
    }
    let a = &request["action"];
    let kind = text(a, "kind")?;
    let allowed: &[&str] = match kind {
        "inventory" | "images" | "host_capacity" | "host_identity" | "storage_status"
        | "host_status" | "universe_stats" => &["kind"],
        "inspect" | "start" | "pause" | "unpause" | "rm" | "cgroup_limits" => &["kind", "uuid"],
        "create" => &["kind", "uuid", "image", "command", "labels", "nested"],
        "stop" => &["kind", "uuid", "timeout_seconds"],
        "signal" => &["kind", "uuid", "signal"],
        "resources" => &["kind", "uuid", "memory_bytes", "cpus"],
        "snapshot" => &["kind", "source_uuid", "reference", "labels"],
        "remove_snapshots" => &["kind", "targets"],
        _ => return Err("unknown host capability".into()),
    };
    fields(a, allowed)?;
    match kind {
        "host_identity" => return host_identity_local(),
        "host_capacity" => return host_capacity(),
        "storage_status" => return crate::storage::status(None),
        "host_status" => return crate::health::host_status(),
        "universe_stats" => {
            return Err("host universe statistics not ported to adapter scope".into())
        }
        "images" => {
            return Ok(
                json!({"exit_code":0,"stdout":serde_json::to_string(&policy.images()?)?,"stderr":""}),
            )
        }
        "inventory" => {
            let out = command(
                30,
                &[
                    "ps".into(),
                    "--all".into(),
                    "--format".into(),
                    "json".into(),
                ],
            )?;
            if !out.status.success() {
                return Err("host inventory unavailable".into());
            }
            let rows: Vec<Value> = serde_json::from_slice(&out.stdout)?;
            let mut owned = Vec::new();
            for row in rows {
                let id = row["Names"].as_array().and_then(|v| {
                    v.iter()
                        .filter_map(Value::as_str)
                        .find_map(|name| named(name).ok())
                });
                if let Some(id) = id {
                    if policy.record_path(id)?.exists() {
                        policy.owned(id)?;
                        owned.push(row);
                    }
                }
            }
            return Ok(json!({"exit_code":0,"stdout":serde_json::to_string(&owned)?,"stderr":""}));
        }
        "inspect" => {
            let id = text(a, "uuid")?;
            let containers = if policy.inspect(id)?.is_none() {
                vec![]
            } else {
                vec![policy.owned(id)?]
            };
            return Ok(
                json!({"exit_code":0,"stdout":serde_json::to_string(&containers)?,"stderr":""}),
            );
        }
        "cgroup_limits" => {
            let c = policy.owned(text(a, "uuid")?)?;
            return Ok(crate::lifecycle::cgroup_limits_local(&c).unwrap_or(Value::Null));
        }
        _ => {}
    }
    let intent = &request["intent"];
    let (id, target) = validate_intent(a, intent)?;
    if !policy
        .operations
        .iter()
        .any(|v| intent["operation"] == v.as_str())
    {
        return Err("operation outside host-owned policy".into());
    }
    policy.bind_intent(intent)?;
    let mut args: Vec<String> = Vec::new();
    let mut limit = 30;
    match kind {
        "create" => {
            let labels = &a["labels"];
            fields(
                labels,
                &[
                    "io.podmesh.universe",
                    "io.podmesh.creation-operation",
                    "io.podmesh.network-profile",
                    "io.podmesh.universe-profile",
                ],
            )?;
            if labels["io.podmesh.universe"] != target
                || labels["io.podmesh.creation-operation"] != id
                || labels["io.podmesh.network-profile"] != "isolated"
            {
                return Err("create labels are outside isolated operation binding".into());
            }
            let image = digest(text(a, "image")?)?;
            let command_tokens = tokens(&a["command"])?;
            if intent["operation"] == "create" {
                if digest(text(intent, "image")?)? != image
                    || intent["command"] != a["command"]
                    || intent["network_profile"] != "isolated"
                    || intent.get("secrets").is_some()
                    || intent.get("manager_host_state").is_some()
                {
                    return Err("create payload is outside committed isolated command".into());
                }
                if !policy.images.iter().any(|i| digest(i).ok() == Some(image)) {
                    return Err("image is outside host-owned policy".into());
                }
            } else {
                if !command_tokens.is_empty() {
                    return Err("clone cannot override snapshot command".into());
                }
                let allowed = policy.images()?.into_iter().any(|i| {
                    i["Id"].as_str().and_then(|v| digest(v).ok()) == Some(image)
                        && i["Labels"][SCOPE_LABEL] == policy.scope
                        && i["Labels"]["io.podmesh.snapshot-for"] == target
                        && i["Labels"]["io.podmesh.snapshot-operation"] == id
                });
                if !allowed {
                    return Err("clone image is outside owned snapshot binding".into());
                }
            }
            let nested = a
                .get("nested")
                .map(|v| v.as_bool().ok_or("nested capability must be boolean"))
                .transpose()?
                .unwrap_or(false);
            let profile = labels["io.podmesh.universe-profile"]
                .as_str()
                .unwrap_or("flat");
            if !matches!(profile, "flat" | "nested") || nested != (profile == "nested") {
                return Err("nested capability differs from typed universe profile".into());
            }
            if intent["operation"] == "create" {
                if intent["universe_profile"].as_str().unwrap_or("flat") != profile {
                    return Err("universe profile differs from committed operation".into());
                }
                if nested {
                    policy.require_nested(target, image)?;
                }
            } else {
                let source = policy.owned(text(intent, "source_uuid")?)?;
                let source_nested =
                    source["Config"]["Labels"]["io.podmesh.universe-profile"] == "nested";
                if source_nested != nested {
                    return Err("clone profile differs from owned source".into());
                }
                if nested {
                    let source_record: Value = serde_json::from_slice(&fs::read(
                        policy.record_path(text(intent, "source_uuid")?)?,
                    )?)?;
                    let base = text(&source_record, "base_image")?;
                    policy.require_nested(target, base)?;
                }
            }
            use sha2::{Digest, Sha256};
            let action_hash = format!("{:x}", Sha256::digest(serde_json::to_vec(a)?));
            let record_path = policy.record_path(target)?;
            if record_path.exists() {
                let previous: Value = serde_json::from_slice(&fs::read(&record_path)?)?;
                if previous["action_sha256"] != action_hash || previous["operation_id"] != id {
                    return Err("universe already belongs to another adapter creation".into());
                }
            } else {
                let records = fs::read_dir(&policy.state)?
                    .filter_map(Result::ok)
                    .filter(|e| {
                        e.path()
                            .file_stem()
                            .and_then(|v| v.to_str())
                            .is_some_and(crate::lifecycle::is_uuid)
                    })
                    .count() as u64;
                if records >= policy.max_universes {
                    return Err("adapter universe identity budget exhausted; retained history is not silently discarded".into());
                }
                if policy.inspect(target)?.is_some() {
                    return Err("universe name already exists outside adapter ownership".into());
                }
                policy.write_record(
                    target,
                    &json!({"operation_id":id,"action_sha256":action_hash,"container_id":null,"base_image":if intent["operation"] == "create" { json!(image) } else { let source_record: Value = serde_json::from_slice(&fs::read(policy.record_path(text(intent,"source_uuid")?)?)?)?; source_record["base_image"].clone() }}),
                )?;
            }
            if policy.inspect(target)?.is_some() {
                let c = policy.owned(target)?;
                verify_created_envelope(&c, nested)?;
                return Ok(json!({"exit_code":0,"stdout":text(&c,"Id")?,"stderr":""}));
            }
            let record: Value = serde_json::from_slice(&fs::read(policy.record_path(target)?)?)?;
            if record["container_id"].is_string() {
                return Err(
                    "previously observed universe absent; do not recreate its bound identity"
                        .into(),
                );
            }
            args = vec![
                "create".into(),
                "--pull=never".into(),
                "--network=none".into(),
                "--image-volume=ignore".into(),
                "--pid=private".into(),
                "--ipc=private".into(),
                "--uts=private".into(),
                "--memory".into(),
                policy.max_memory.to_string(),
                "--memory-swap".into(),
                policy.max_memory.to_string(),
                "--cpus".into(),
                policy.max_cpus.to_string(),
                "--name".into(),
                format!("podmesh-{target}"),
            ];
            if nested {
                args.push("--privileged".into());
            } else {
                args.extend([
                    "--cap-drop=ALL".into(),
                    "--security-opt=no-new-privileges".into(),
                ]);
            }
            for (key, value) in labels.as_object().ok_or("labels required")? {
                args.extend([
                    "--label".into(),
                    format!("{key}={}", value.as_str().ok_or("labels must be text")?),
                ]);
            }
            args.extend([
                "--label".into(),
                format!("{SCOPE_LABEL}={}", policy.scope),
                "--label".into(),
                format!("{ACTION_LABEL}={action_hash}"),
                format!("sha256:{image}"),
            ]);
            args.extend(command_tokens);
        }
        "snapshot" => {
            let source = text(a, "source_uuid")?;
            let c = policy.owned(source)?;
            if !crate::lifecycle::STOPPED.contains(&crate::lifecycle::status(&c))
                || !c["Mounts"].as_array().is_some_and(Vec::is_empty)
            {
                return Err("clone source must be stopped and mount-free".into());
            }
            let labels = &a["labels"];
            fields(
                labels,
                &[
                    "io.podmesh.snapshot-for",
                    "io.podmesh.snapshot-operation",
                    "io.podmesh.snapshot-source-container",
                ],
            )?;
            if labels["io.podmesh.snapshot-for"] != target
                || labels["io.podmesh.snapshot-operation"] != id
                || labels["io.podmesh.snapshot-source-container"] != c["Id"]
            {
                return Err("snapshot provenance differs from owned source".into());
            }
            let reference = format!("localhost/podmesh-clone:{id}");
            if text(a, "reference")? != reference {
                return Err("snapshot reference outside operation binding".into());
            }
            args = vec!["commit".into(), "--pause=false".into()];
            for (key, value) in labels.as_object().ok_or("snapshot labels required")? {
                args.extend([
                    "--change".into(),
                    format!(
                        "LABEL {key}={}",
                        value.as_str().ok_or("snapshot label must be text")?
                    ),
                ]);
            }
            args.extend([
                "--change".into(),
                format!("LABEL {SCOPE_LABEL}={}", policy.scope),
                format!("podmesh-{source}"),
                reference,
            ]);
            limit = 300;
        }
        "remove_snapshots" => {
            let images = policy.images()?;
            args = vec!["image".into(), "rm".into()];
            for target_image in tokens(&a["targets"])? {
                if !images.iter().any(|i| {
                    i["Labels"][SCOPE_LABEL] == policy.scope
                        && i["Labels"]["io.podmesh.snapshot-for"] == target
                        && (i["Id"].as_str().and_then(|v| digest(v).ok())
                            == digest(&target_image).ok()
                            || i["Names"]
                                .as_array()
                                .is_some_and(|n| n.contains(&json!(target_image))))
                }) {
                    return Err("image removal is outside owned snapshot scope".into());
                }
                args.push(target_image);
            }
            if args.len() == 2 {
                return Err("snapshot removal needs owned targets".into());
            }
        }
        _ => {
            let c = policy.owned(target)?;
            let name = format!("podmesh-{target}");
            match kind {
                "rm" => {
                    if !crate::lifecycle::STOPPED.contains(&crate::lifecycle::status(&c)) {
                        return Err("adapter delete requires stopped owned container".into());
                    }
                    args = vec!["rm".into(), name];
                }
                "start" | "pause" | "unpause" => args = vec![kind.into(), name],
                "stop" => {
                    let seconds = a["timeout_seconds"]
                        .as_u64()
                        .filter(|n| *n <= 300)
                        .ok_or("stop timeout outside bounds")?;
                    if a["timeout_seconds"] != intent["timeout_seconds"]
                        || intent["on_timeout"] != "kill"
                    {
                        return Err("stop escalation differs from committed operation".into());
                    }
                    args = vec!["stop".into(), "--time".into(), seconds.to_string(), name];
                    limit = seconds + 30;
                }
                "signal" => {
                    let signal = c["Config"]["StopSignal"].as_str().unwrap_or("SIGTERM");
                    if text(a, "signal")? != signal
                        || signal.len() > 32
                        || !signal.bytes().all(|b| b.is_ascii_alphanumeric())
                        || intent["on_timeout"] != "leave_running"
                    {
                        return Err("stop signal differs from owned container or operation".into());
                    }
                    args = vec!["kill".into(), "--signal".into(), signal.into(), name];
                }
                "resources" => {
                    if a["memory_bytes"] != intent["memory_bytes"] || a["cpus"] != intent["cpus"] {
                        return Err("resource action differs from committed operation".into());
                    }
                    args = vec!["update".into()];
                    if !a["memory_bytes"].is_null() {
                        let m = a["memory_bytes"]
                            .as_u64()
                            .filter(|m| *m >= 32 * 1024 * 1024 && *m <= policy.max_memory)
                            .ok_or("memory outside host policy")?;
                        args.extend([
                            "--memory".into(),
                            m.to_string(),
                            "--memory-swap".into(),
                            m.to_string(),
                        ]);
                    }
                    if !a["cpus"].is_null() {
                        let cp = a["cpus"]
                            .as_f64()
                            .filter(|cp| cp.is_finite() && *cp >= 0.1 && *cp <= policy.max_cpus)
                            .ok_or("CPU outside host policy")?;
                        args.extend(["--cpus".into(), cp.to_string()]);
                    }
                    if args.len() == 1 {
                        return Err("resource action requires a bound".into());
                    }
                    args.push(name);
                }
                _ => return Err("unsupported owned effect".into()),
            }
        }
    }
    let out = command(limit, &args)?;
    if kind == "create" && out.status.success() {
        let observed = policy.owned(target)?;
        verify_created_envelope(&observed, a["nested"].as_bool().unwrap_or(false))?;
    }
    Ok(output(out))
}

/// Host-only server. Policy/state must be prepared explicitly; socket endpoint is never replaced.
pub fn serve(policy_path: &Path, socket: &Path) -> Result<(), Error> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(
            "host capability provider requires its separately authorized root identity".into(),
        );
    }
    if configured() {
        return Err("host provider must not itself configure a client adapter endpoint".into());
    }
    let policy = Policy::load(policy_path)?;
    if !socket.is_absolute() {
        return Err("host socket must name an absolute path".into());
    }
    protected_ancestors(socket)?;
    let parent = socket.parent().ok_or("host socket parent required")?;
    let metadata = fs::symlink_metadata(parent)?;
    if !metadata.is_dir()
        || metadata.uid() != 0
        || metadata.gid() != policy.gid
        || metadata.permissions().mode() & 0o022 != 0
    {
        return Err("host socket parent must be root-owned and not group/world writable".into());
    }
    let listener = UnixListener::bind(socket)?;
    let path = std::ffi::CString::new(socket.as_os_str().as_encoded_bytes())?;
    if unsafe { libc::chown(path.as_ptr(), 0, policy.gid) } != 0 {
        return Err("cannot set host socket group".into());
    }
    fs::set_permissions(socket, fs::Permissions::from_mode(0o660))?;
    for stream in listener.incoming() {
        let mut stream = stream?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let result = (|| -> Result<Value, Error> {
            if peer_uid(&stream)? != policy.uid {
                return Err("host capability peer is not the authorized application".into());
            }
            let mut bytes = Vec::new();
            BufReader::new(&stream)
                .take(MAX_REQUEST + 1)
                .read_until(b'\n', &mut bytes)?;
            if bytes.len() as u64 > MAX_REQUEST || !bytes.ends_with(b"\n") {
                return Err("incomplete or oversized host request".into());
            }
            execute(&policy, &serde_json::from_slice(&bytes)?)
        })();
        let response = match result {
            Ok(data) => json!({"ok":true,"data":data}),
            Err(error) => json!({"ok":false,"error":error.to_string()}),
        };
        let _ = writeln!(stream, "{response}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const U: &str = "24e5b403-0fbf-43bf-b657-fd54277a2bda";
    fn intent(operation: &str) -> Value {
        json!({"operation":operation,"operation_id":"typed-fixture","universe_uuid":U,"authorization_ref":"bounded engineering mandate"})
    }
    #[test]
    fn observed_mounts_privilege_and_namespace_must_match_fixed_envelope() {
        let valid = json!({"HostConfig":{"Privileged":false,"NetworkMode":"none","PidMode":"private","IpcMode":"private","UTSMode":"private"},"Mounts":[]});
        assert!(verify_created_envelope(&valid, false).is_ok());
        assert!(verify_created_envelope(&valid, true).is_err());
        let mut invalid = valid.clone();
        invalid["Mounts"] = json!([{"Source":"/host"}]);
        assert!(verify_created_envelope(&invalid, false).is_err());
        for field in ["PidMode", "IpcMode", "UTSMode", "NetworkMode"] {
            let mut invalid = valid.clone();
            invalid["HostConfig"][field] = json!("host");
            assert!(verify_created_envelope(&invalid, false).is_err());
        }
    }
    #[test]
    fn host_boot_observation_requires_valid_identity_and_preserves_unknown_clock() {
        let valid = json!({"machine_id":"a".repeat(32),"boot_id":U,"booted_at":1234,"clock_synchronized":null});
        let host = HostIdentity::decode(&valid).unwrap();
        assert_eq!(host.boot, U);
        assert_eq!(host.booted, Some(1234));
        assert_eq!(host.clock, None);
        for (field, value) in [
            ("boot_id", json!("invalid")),
            ("machine_id", json!("")),
            ("booted_at", json!(-1)),
            ("clock_synchronized", json!("yes")),
        ] {
            let mut invalid = valid.clone();
            invalid[field] = value;
            assert!(HostIdentity::decode(&invalid).is_err());
        }
    }
    #[test]
    fn nested_conversion_requires_profile_and_never_accepts_host_mounts() {
        let name = format!("podmesh-{U}");
        let image = format!("sha256:{}", "a".repeat(64));
        let profile = "io.podmesh.universe-profile=nested";
        let action = typed(&[
            "create",
            "--pull=never",
            "--privileged",
            "--network=none",
            "--name",
            &name,
            "--label",
            profile,
            &image,
            "sleep",
            "60",
        ])
        .unwrap();
        assert_eq!(action["nested"], true);
        assert!(typed(&[
            "create",
            "--pull=never",
            "--privileged",
            "--network=none",
            "--name",
            &name,
            "--label",
            profile,
            "--volume",
            "/:/host",
            &image
        ])
        .is_err());
        assert!(typed(&[
            "create",
            "--pull=never",
            "--privileged",
            "--network=none",
            "--name",
            &name,
            &image
        ])
        .is_err());
    }
    #[test]
    fn lifecycle_arguments_cross_as_typed_fields_without_flags() {
        let name = format!("podmesh-{U}");
        for (kind, args) in [
            ("start", vec!["start", name.as_str()]),
            ("pause", vec!["pause", name.as_str()]),
            ("unpause", vec!["unpause", name.as_str()]),
            ("rm", vec!["rm", name.as_str()]),
        ] {
            let a = typed(&args).unwrap();
            assert_eq!(a["kind"], kind);
            assert_eq!(a["uuid"], U);
            assert!(a.get("args").is_none());
        }
        let a = typed(&["stop", "--time", "5", &name]).unwrap();
        assert_eq!(a["timeout_seconds"], 5);
        let a = typed(&[
            "update",
            "--memory",
            "33554432",
            "--memory-swap",
            "33554432",
            "--cpus",
            "0.5",
            &name,
        ])
        .unwrap();
        assert_eq!(a["memory_bytes"], 33554432u64);
        assert_eq!(a["cpus"], 0.5);
    }
    #[test]
    fn arbitrary_names_flags_paths_and_shell_commands_refuse() {
        for args in [
            vec!["start", "database"],
            vec!["start", "../podmesh-x"],
            vec!["rm", "--force", "database"],
            vec!["exec", "database", "sh"],
            vec!["pull", "image"],
            vec!["run", "--privileged", "image"],
            vec!["volume", "rm", "volume"],
            vec![
                "create",
                "--pull=never",
                "--privileged",
                "--network=none",
                "image",
            ],
        ] {
            assert!(typed(&args).is_err(), "{args:?}");
        }
        let name = format!("podmesh-{U}");
        assert!(typed(&["update", "--memory", "32", "--memory-swap", "64", &name]).is_err());
    }
    #[test]
    fn create_conversion_requires_local_isolated_shape() {
        let name = format!("podmesh-{U}");
        let image = format!("sha256:{}", "a".repeat(64));
        let universe = format!("io.podmesh.universe={U}");
        let a = typed(&[
            "create",
            "--pull=never",
            "--network=none",
            "--name",
            &name,
            "--label",
            &universe,
            &image,
            "sleep",
            "60",
        ])
        .unwrap();
        assert_eq!(a["command"], json!(["sleep", "60"]));
        assert_eq!(a["image"], "a".repeat(64));
        assert!(typed(&[
            "create",
            "--pull=never",
            "--network=host",
            "--name",
            &name,
            &image
        ])
        .is_err());
        assert!(typed(&[
            "create",
            "--pull=never",
            "--network=none",
            "--name",
            &name,
            "--volume",
            "/:/host",
            &image
        ])
        .is_err());
    }
    #[test]
    fn effects_bind_operation_uuid_source_and_mandate() {
        let a = json!({"kind":"start","uuid":U});
        assert!(validate_intent(&a, &intent("start")).is_ok());
        assert!(validate_intent(&a, &intent("delete")).is_err());
        assert!(validate_intent(&a, &Value::Null).is_err());
        let mut wrong = intent("start");
        wrong["universe_uuid"] = json!("934e4b6e-adf1-497e-9fb7-b071ff584ce4");
        assert!(validate_intent(&a, &wrong).is_err());
        wrong = intent("start");
        wrong["authorization_ref"] = json!("");
        assert!(validate_intent(&a, &wrong).is_err());
        let snapshot = json!({"kind":"snapshot","source_uuid":U});
        assert!(validate_intent(&snapshot, &intent("clone")).is_err());
    }
    #[test]
    fn intent_context_is_scoped_and_restored_on_nested_calls() {
        assert!(INTENT.with(|v| v.borrow().is_none()));
        let first = intent("start");
        let second = intent("stop");
        with_intent(&first, || {
            assert_eq!(
                INTENT.with(|v| v.borrow().clone()).unwrap()["operation"],
                "start"
            );
            with_intent(&second, || {
                assert_eq!(
                    INTENT.with(|v| v.borrow().clone()).unwrap()["operation"],
                    "stop"
                )
            });
            assert_eq!(
                INTENT.with(|v| v.borrow().clone()).unwrap()["operation"],
                "start"
            );
        });
        assert!(INTENT.with(|v| v.borrow().is_none()));
    }
    #[test]
    fn peer_identity_is_observed_from_kernel_not_request() {
        let (a, b) = UnixStream::pair().unwrap();
        let uid = unsafe { libc::geteuid() };
        assert_eq!(peer_uid(&a).unwrap(), uid);
        assert_eq!(peer_uid(&b).unwrap(), uid);
    }
    #[test]
    fn nested_authority_is_exact_uuid_and_base_image() {
        let p = Policy {
            scope: "fixture".into(),
            uid: 1102,
            gid: 1102,
            images: vec![],
            nested: vec![(U.into(), "a".repeat(64))],
            operations: vec![],
            max_memory: 33554432,
            max_cpus: 1.0,
            max_universes: 4,
            state: PathBuf::from("/unused"),
        };
        assert!(p.require_nested(U, &"a".repeat(64)).is_ok());
        assert!(p.require_nested(U, &"b".repeat(64)).is_err());
        assert!(p
            .require_nested("934e4b6e-adf1-497e-9fb7-b071ff584ce4", &"a".repeat(64))
            .is_err());
    }
    #[test]
    fn unknown_fields_versions_and_effects_refuse_before_host_commands() {
        let p = Policy {
            scope: "fixture".into(),
            uid: 1102,
            gid: 1102,
            images: vec![],
            nested: vec![],
            operations: vec!["start".into()],
            max_memory: 33554432,
            max_cpus: 1.0,
            max_universes: 4,
            state: PathBuf::from("/must-not-be-used"),
        };
        for request in [
            json!({"protocol":"wrong","action":{"kind":"start","uuid":U},"intent":intent("start")}),
            json!({"protocol":PROTOCOL,"action":{"kind":"shell","command":"id"},"intent":intent("start")}),
            json!({"protocol":PROTOCOL,"action":{"kind":"start","uuid":U,"args":["--privileged"]},"intent":intent("start")}),
            json!({"protocol":PROTOCOL,"action":{"kind":"start","uuid":U},"intent":intent("delete")}),
        ] {
            assert!(execute(&p, &request).is_err());
        }
    }
}

#[cfg(test)]
mod real_effect_tests {
    use super::*;
    /// Real host-effect engineering test, explicitly armed with an already-local immutable image.
    /// It does not replace private-app+MariaDB+peer/restore qualification on the runtime hosts.
    #[test]
    fn real_scoped_provider_lifecycle_from_empty_when_a_local_image_is_named() {
        exercise(false);
    }
    #[test]
    fn real_scoped_nested_provider_lifecycle_when_explicitly_named() {
        exercise(true);
    }
    fn exercise(nested: bool) {
        let fixture = if nested {
            "PODMESH_HOST_ADAPTER_TEST_NESTED_IMAGE"
        } else {
            "PODMESH_HOST_ADAPTER_TEST_IMAGE"
        };
        let Ok(image) = std::env::var(fixture) else {
            eprintln!("skipped: {fixture} names no isolated host fixture image");
            return;
        };
        assert_eq!(
            unsafe { libc::geteuid() },
            0,
            "named host fixture requires authorized host-provider identity"
        );
        digest(&image).expect("named fixture must be a full already-local image digest");
        let nonce = fs::read_to_string("/proc/sys/kernel/random/uuid")
            .unwrap()
            .trim()
            .to_string();
        let child = fs::read_to_string("/proc/sys/kernel/random/uuid")
            .unwrap()
            .trim()
            .to_string();
        let state = std::env::temp_dir().join(format!("podmesh-host-fixture-{nonce}"));
        fs::create_dir(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        let policy = Policy {
            scope: format!("fixture-{nonce}"),
            uid: 1102,
            gid: 1102,
            images: vec![image.clone()],
            nested: if nested {
                vec![
                    (nonce.clone(), digest(&image).unwrap().into()),
                    (child.clone(), digest(&image).unwrap().into()),
                ]
            } else {
                vec![]
            },
            operations: vec![
                "create",
                "start",
                "resources",
                "pause",
                "resume",
                "stop",
                "clone",
                "delete",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
            max_memory: 64 * 1024 * 1024,
            max_cpus: 0.5,
            max_universes: 4,
            state,
        };
        struct Cleanup<'a> {
            policy: &'a Policy,
            ids: Vec<String>,
        }
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                for id in &self.ids {
                    let _ = command(30, &["rm".into(), "--force".into(), id.clone()]);
                }
                if let Ok(images) = self.policy.images() {
                    for image in images {
                        if image["Labels"][SCOPE_LABEL] == self.policy.scope {
                            if let Some(id) = image["Id"].as_str() {
                                let _ = command(30, &["image".into(), "rm".into(), id.into()]);
                            }
                        }
                    }
                }
                let _ = fs::remove_dir_all(&self.policy.state);
            }
        }
        let mut cleanup = Cleanup {
            policy: &policy,
            ids: Vec::new(),
        };
        let request = |operation: &str, id: &str, target: &str| json!({"operation":operation,"operation_id":id,"universe_uuid":target,"authorization_ref":"isolated named host engineering fixture"});
        let invoke = |action: Value, intent: Value| {
            let data = execute(
                &policy,
                &json!({"protocol":PROTOCOL,"action":action,"intent":intent}),
            )
            .unwrap();
            assert_eq!(data["exit_code"], 0, "{data}");
            data
        };
        let absent = invoke(json!({"kind":"inspect","uuid":nonce}), Value::Null);
        assert_eq!(
            serde_json::from_str::<Value>(absent["stdout"].as_str().unwrap()).unwrap(),
            json!([])
        );
        let create_id = format!("create-{nonce}");
        let mut create = request("create", &create_id, &nonce);
        create["image"] = json!(image);
        create["command"] = json!(["sleep", "60"]);
        create["network_profile"] = json!("isolated");
        create["universe_profile"] = json!(if nested { "nested" } else { "flat" });
        let create_action = json!({"kind":"create","uuid":nonce,"image":digest(&image).unwrap(),"command":["sleep","60"],"nested":nested,"labels":{"io.podmesh.universe":nonce,"io.podmesh.creation-operation":create_id,"io.podmesh.network-profile":"isolated","io.podmesh.universe-profile":if nested { "nested" } else { "flat" }}});
        invoke(create_action.clone(), create.clone());
        let container = policy.owned(&nonce).unwrap();
        cleanup.ids.push(container["Id"].as_str().unwrap().into());
        assert_eq!(container["HostConfig"]["Privileged"], nested);
        invoke(create_action, create.clone());
        assert_eq!(policy.owned(&nonce).unwrap()["Id"], container["Id"]);
        let mut changed = create.clone();
        changed["authorization_ref"] = json!("different mandate");
        assert!(policy.bind_intent(&changed).is_err());
        invoke(
            json!({"kind":"start","uuid":nonce}),
            request("start", &format!("start-{nonce}"), &nonce),
        );
        assert_eq!(policy.owned(&nonce).unwrap()["State"]["Running"], true);
        let mut resources = request("resources", &format!("resources-{nonce}"), &nonce);
        resources["memory_bytes"] = json!(32 * 1024 * 1024);
        resources["cpus"] = json!(0.25);
        invoke(
            json!({"kind":"resources","uuid":nonce,"memory_bytes":32*1024*1024,"cpus":0.25}),
            resources,
        );
        let kernel=execute(&policy,&json!({"protocol":PROTOCOL,"action":{"kind":"cgroup_limits","uuid":nonce},"intent":null})).unwrap();
        assert_eq!(kernel["memory_max_bytes"], 32 * 1024 * 1024);
        assert_eq!(kernel["cpus"], 0.25);
        invoke(
            json!({"kind":"pause","uuid":nonce}),
            request("pause", &format!("pause-{nonce}"), &nonce),
        );
        assert_eq!(policy.owned(&nonce).unwrap()["State"]["Status"], "paused");
        invoke(
            json!({"kind":"unpause","uuid":nonce}),
            request("resume", &format!("resume-{nonce}"), &nonce),
        );
        assert_eq!(policy.owned(&nonce).unwrap()["State"]["Running"], true);
        let mut stop = request("stop", &format!("stop-{nonce}"), &nonce);
        stop["timeout_seconds"] = json!(1);
        stop["on_timeout"] = json!("kill");
        invoke(
            json!({"kind":"stop","uuid":nonce,"timeout_seconds":1}),
            stop,
        );
        assert_eq!(policy.owned(&nonce).unwrap()["State"]["Running"], false);
        let clone_id = format!("clone-{nonce}");
        let mut clone = request("clone", &clone_id, &child);
        clone["source_uuid"] = json!(nonce);
        let reference = format!("localhost/podmesh-clone:{clone_id}");
        invoke(
            json!({"kind":"snapshot","source_uuid":nonce,"reference":reference,"labels":{"io.podmesh.snapshot-for":child,"io.podmesh.snapshot-operation":clone_id,"io.podmesh.snapshot-source-container":container["Id"]}}),
            clone.clone(),
        );
        let snapshot = policy
            .images()
            .unwrap()
            .into_iter()
            .find(|i| i["Labels"]["io.podmesh.snapshot-operation"] == clone_id)
            .unwrap();
        let snapshot_id = snapshot["Id"].as_str().unwrap();
        invoke(
            json!({"kind":"create","uuid":child,"image":digest(snapshot_id).unwrap(),"command":[],"nested":nested,"labels":{"io.podmesh.universe":child,"io.podmesh.creation-operation":clone_id,"io.podmesh.network-profile":"isolated","io.podmesh.universe-profile":if nested { "nested" } else { "flat" }}}),
            clone,
        );
        cleanup
            .ids
            .push(policy.owned(&child).unwrap()["Id"].as_str().unwrap().into());
        let delete = request("delete", &format!("delete-child-{nonce}"), &child);
        invoke(json!({"kind":"rm","uuid":child}), delete.clone());
        invoke(
            json!({"kind":"remove_snapshots","targets":[reference]}),
            delete,
        );
        assert!(policy.inspect(&child).unwrap().is_none());
        invoke(
            json!({"kind":"rm","uuid":nonce}),
            request("delete", &format!("delete-{nonce}"), &nonce),
        );
        assert!(policy.inspect(&nonce).unwrap().is_none());
        // Model an interrupted host create with correct ownership labels but a wrong
        // namespace. Observing/refusing it must not turn into success on retry.
        let invalid_uuid = fs::read_to_string("/proc/sys/kernel/random/uuid")
            .unwrap()
            .trim()
            .to_string();
        let invalid_id = format!("invalid-{invalid_uuid}");
        let mut invalid_intent = request("create", &invalid_id, &invalid_uuid);
        invalid_intent["image"] = json!(image);
        invalid_intent["command"] = json!(["true"]);
        invalid_intent["network_profile"] = json!("isolated");
        let invalid_action = json!({"kind":"create","uuid":invalid_uuid,"image":digest(&image).unwrap(),"command":["true"],"labels":{"io.podmesh.universe":invalid_uuid,"io.podmesh.creation-operation":invalid_id,"io.podmesh.network-profile":"isolated","io.podmesh.universe-profile":"flat"}});
        use sha2::{Digest, Sha256};
        let hash = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&invalid_action).unwrap())
        );
        policy.write_record(&invalid_uuid, &json!({"operation_id":invalid_id,"action_sha256":hash,"container_id":null,"base_image":digest(&image).unwrap()})).unwrap();
        let malformed = command(
            30,
            &[
                "create".into(),
                "--pull=never".into(),
                "--network=none".into(),
                "--image-volume=ignore".into(),
                "--ipc=host".into(),
                "--name".into(),
                format!("podmesh-{invalid_uuid}"),
                "--label".into(),
                format!("{SCOPE_LABEL}={}", policy.scope),
                "--label".into(),
                format!("{ACTION_LABEL}={hash}"),
                "--label".into(),
                format!("io.podmesh.creation-operation={invalid_id}"),
                "--label".into(),
                format!("io.podmesh.universe={invalid_uuid}"),
                "--label".into(),
                "io.podmesh.universe-profile=flat".into(),
                image.clone(),
                "true".into(),
            ],
        )
        .unwrap();
        assert!(malformed.status.success());
        cleanup
            .ids
            .push(String::from_utf8(malformed.stdout).unwrap().trim().into());
        for _ in 0..2 {
            let refused = execute(
                &policy,
                &json!({"protocol":PROTOCOL,"action":invalid_action,"intent":invalid_intent}),
            )
            .unwrap_err();
            assert!(refused.to_string().contains("private namespace envelope"));
        }
        assert!(
            policy.inspect(&invalid_uuid).unwrap().is_some(),
            "refusal never removes a container"
        );
        // Replacing a previously bound container name cannot inherit adapter ownership.
        let replace = command(
            30,
            &[
                "create".into(),
                "--pull=never".into(),
                "--network=none".into(),
                "--name".into(),
                format!("podmesh-{nonce}"),
                image,
                "true".into(),
            ],
        )
        .unwrap();
        assert!(replace.status.success());
        cleanup.ids.push(
            String::from_utf8(replace.stdout)
                .unwrap()
                .trim()
                .to_string(),
        );
        assert!(policy.owned(&nonce).is_err());
    }
}
