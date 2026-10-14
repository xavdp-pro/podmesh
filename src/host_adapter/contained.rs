//! Host-owned, explicitly contained nested grants. No caller supplies host paths or mappings.
use super::{digest, fields, text, uuid, Error, SCOPE_LABEL};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Component, Path},
};

pub(super) const PROFILE: &str = "userns-v1";
pub(super) const LABEL: &str = "io.podmesh.host-containment";
// Podman's ordinary service capabilities plus the bounded nested-runtime additions.
const CAPS: &[&str] = &[
    "CHOWN",
    "DAC_OVERRIDE",
    "FOWNER",
    "FSETID",
    "KILL",
    "SETGID",
    "SETUID",
    "SETPCAP",
    "NET_BIND_SERVICE",
    "SYS_CHROOT",
    "SETFCAP",
    "NET_RAW",
    "SYS_ADMIN",
    "NET_ADMIN",
    "MKNOD",
    "SYS_RESOURCE",
    "SYS_PTRACE",
    "AUDIT_WRITE",
];

#[derive(Clone, Debug)]
pub(super) struct Grant {
    pub uuid: String,
    pub image: String,
    pub value: Value,
}

fn number(value: &Value, name: &str) -> Result<u64, Error> {
    value[name]
        .as_u64()
        .ok_or_else(|| format!("contained grant requires numeric {name}").into())
}
fn normalized(path: &str) -> bool {
    let p = Path::new(path);
    p.is_absolute()
        && p.components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
        && p.to_string_lossy() == path
        && !path.chars().any(|c| matches!(c, ',' | ':' | '\n' | '\0'))
        && !path.ends_with('/')
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Grant {
        Grant {
            uuid: "24e5b403-0fbf-43bf-b657-fd54277a2bda".into(),
            image: "a".repeat(64),
            value: json!({"universe_uuid":"24e5b403-0fbf-43bf-b657-fd54277a2bda","image":"a".repeat(64),
                "containment":PROFILE,"uid_start":200000,"gid_start":300000,"id_count":65536,
                "volume_root":"/declared/owned/core", "mounts":[], "network":"none"}),
        }
    }
    fn observed() -> Value {
        json!({"Config":{"User":"0:0","Labels":{LABEL:PROFILE}},
            "HostConfig":{"Privileged":false,"UsernsMode":"private", "NetworkMode":"none",
                "PidMode":"private","IpcMode":"private","UTSMode":"private","CgroupMode":"private",
                "Devices":[{"PathOnHost":"/dev/fuse","PathInContainer":"/dev/fuse"},{"PathOnHost":"/dev/net/tun","PathInContainer":"/dev/net/tun"}],
                "IDMappings":{"UidMap":["0:200000:65536"],"GidMap":["0:300000:65536"]}},
            "BoundingCaps":CAPS.iter().map(|cap|format!("CAP_{cap}")).collect::<Vec<_>>(),
            "Mounts":[],"State":{"Running":false,"Paused":false,"Pid":0}})
    }
    #[test]
    fn contained_args_never_grant_host_privilege_or_arbitrary_devices() {
        let args = fixture().arguments();
        assert!(!args
            .iter()
            .any(|v| v == "--privileged" || v == "--userns=host" || v.contains("SYS_MODULE")));
        assert!(args.contains(&"0:200000:65536".into()));
        assert!(args.contains(&"--cap-drop=ALL".into()));
        assert!(args.contains(&"--device=/dev/fuse".into()));
        assert!(args.contains(&"--device=/dev/net/tun".into()));
    }
    #[test]
    fn mappings_and_mount_roots_must_not_overlap_other_grants() {
        let a = fixture();
        let mut b = a.clone();
        b.value["uid_start"] = json!(400000);
        b.value["gid_start"] = json!(500000);
        b.value["volume_root"] = json!("/declared/owned/second");
        assert!(!a.overlaps(&b));
        b.value["gid_start"] = json!(300001);
        assert!(a.overlaps(&b));
        b.value["gid_start"] = json!(500000);
        b.value["volume_root"] = json!("/declared/owned/core/child");
        assert!(a.overlaps(&b));
    }
    #[test]
    fn changed_mapping_caps_profile_or_hidden_mount_refuse() {
        let grant = fixture();
        let c = observed();
        let proof = grant.verify(&c, &grant.binding()).unwrap();
        assert_eq!(proof["configuration_verified"], true);
        assert_eq!(proof["runtime_verified"], false);
        for (pointer, value) in [
            ("/HostConfig/Privileged", json!(true)),
            ("/HostConfig/IDMappings/UidMap", json!(["0:0:65536"])),
            ("/HostConfig/UsernsMode", json!("host")),
            ("/HostConfig/NetworkMode", json!("host")),
            ("/HostConfig/CgroupMode", json!("host")),
            ("/BoundingCaps", json!(["CAP_SYS_MODULE"])),
            (
                "/Mounts",
                json!([{"Type":"bind","Source":"/","Destination":"/host","RW":true,"Options":["idmap"]}]),
            ),
        ] {
            let mut changed = c.clone();
            *changed.pointer_mut(pointer).unwrap() = value;
            assert!(
                grant.verify(&changed, &grant.binding()).is_err(),
                "{pointer}"
            );
        }
        let mut changed = grant.binding();
        changed["profile"] = json!("legacy-host-root");
        assert!(grant.verify(&c, &changed).is_err());
    }
    #[test]
    fn invalid_ranges_and_unrecognized_policy_inputs_refuse_before_paths() {
        let mut raw = fixture().value;
        raw["uid_start"] = json!(0);
        assert!(Grant::parse(&raw).is_err());
        raw["uid_start"] = json!(u32::MAX);
        assert!(Grant::parse(&raw).is_err());
        raw["uid_start"] = json!(200000);
        raw["shell"] = json!("id");
        assert!(Grant::parse(&raw).is_err());
        assert!(!normalized("/var/lib/owned/../foreign"));
        assert!(!normalized("/var/lib/owned:path"));
        assert!(!normalized("/var/lib/owned/"));
    }
}
fn directory(path: &str) -> Result<(), Error> {
    if !normalized(path) {
        return Err("contained volume requires a normalized absolute directory".into());
    }
    let p = Path::new(path);
    for part in p.ancestors() {
        let m = fs::symlink_metadata(part)?;
        if !m.is_dir() || m.uid() != 0 || m.permissions().mode() & 0o022 != 0 {
            return Err(
                "contained volumes require protected root-owned directory ancestors".into(),
            );
        }
    }
    if fs::canonicalize(p)? != p {
        return Err("contained source directory may not traverse links".into());
    }
    Ok(())
}
impl Grant {
    pub fn parse(value: &Value) -> Result<Self, Error> {
        fields(
            value,
            &[
                "universe_uuid",
                "image",
                "containment",
                "uid_start",
                "gid_start",
                "id_count",
                "volume_root",
                "mounts",
                "network",
            ],
        )?;
        if text(value, "containment")? != PROFILE {
            return Err("unknown contained nested profile".into());
        }
        let target = text(value, "universe_uuid")?;
        uuid(target)?;
        let image = digest(text(value, "image")?)?;
        let count = number(value, "id_count")?;
        if !(65536..=1048576).contains(&count) {
            return Err("contained ID range must cover bounded inner service identities".into());
        }
        for key in ["uid_start", "gid_start"] {
            let start = number(value, key)?;
            if start < 65536
                || start
                    .checked_add(count)
                    .filter(|end| *end <= u32::MAX as u64)
                    .is_none()
            {
                return Err(
                    "contained IDs must exclude host system identities and fit the kernel range"
                        .into(),
                );
            }
        }
        let root = text(value, "volume_root")?;
        directory(root)?;
        // A dedicated root is mandatory, not a grant to arbitrary system directories.
        if Path::new(root).components().count() < 4 {
            return Err("contained volume root must be a dedicated subtree".into());
        }
        let mounts = value["mounts"]
            .as_array()
            .ok_or("contained mounts must be an array")?;
        if mounts.len() > 16 {
            return Err("contained mount budget exceeded".into());
        }
        let mut destinations = BTreeSet::new();
        let mut sources = BTreeSet::new();
        for mount in mounts {
            fields(mount, &["source", "destination", "read_only"])?;
            let source = text(mount, "source")?;
            let destination = text(mount, "destination")?;
            directory(source)?;
            if source == root || !Path::new(source).starts_with(root) || !sources.insert(source) {
                return Err(
                    "contained mount source must be a unique child of its dedicated root".into(),
                );
            }
            if !normalized(destination)
                || destination == "/"
                || !destinations.insert(destination)
                || ["/proc", "/sys", "/dev", "/run/podman", "/run/systemd"]
                    .iter()
                    .any(|p| Path::new(destination).starts_with(p))
                || mount["read_only"].as_bool().is_none()
            {
                return Err("contained destination/access must be explicit and may not replace a host-control boundary".into());
            }
        }
        // Nested/overlapping binds would hide a declared resource and complicate its recovery.
        for a in &destinations {
            for b in &destinations {
                if a != b && Path::new(b).starts_with(a) {
                    return Err("overlapping contained mount destinations".into());
                }
            }
        }
        for a in &sources {
            for b in &sources {
                if a != b && Path::new(b).starts_with(a) {
                    return Err("overlapping contained mount sources".into());
                }
            }
        }
        let network = text(value, "network")?;
        if network != "none"
            && (network.len() > 128
                || !network
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b)))
        {
            return Err(
                "contained network must be none or an explicitly named scoped network".into(),
            );
        }
        if ["host", "bridge", "private", "slirp4netns", "pasta"].contains(&network) {
            return Err(
                "contained network may not use an implicit runtime or host namespace".into(),
            );
        }
        Ok(Self {
            uuid: target.into(),
            image: image.into(),
            value: value.clone(),
        })
    }
    pub fn overlaps(&self, other: &Self) -> bool {
        ["uid_start", "gid_start"].iter().any(|key| {
            let a = self.value[key].as_u64().unwrap();
            let b = other.value[key].as_u64().unwrap();
            a < b + other.value["id_count"].as_u64().unwrap()
                && b < a + self.value["id_count"].as_u64().unwrap()
        }) || Path::new(self.value["volume_root"].as_str().unwrap())
            .starts_with(other.value["volume_root"].as_str().unwrap())
            || Path::new(other.value["volume_root"].as_str().unwrap())
                .starts_with(self.value["volume_root"].as_str().unwrap())
    }
    pub fn binding(&self) -> Value {
        // A path alone cannot authorize adoption of a replaced directory after provider restart.
        let mounts: Vec<Value> = self.value["mounts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| {
                let metadata = fs::symlink_metadata(m["source"].as_str().unwrap());
                match metadata {
                    Ok(s) => json!({"source":m["source"], "device":s.dev(), "inode":s.ino()}),
                    Err(_) => Value::Null,
                }
            })
            .collect();
        json!({"profile":PROFILE, "grant":self.value, "source_identities":mounts})
    }
    pub fn arguments(&self) -> Vec<String> {
        let count = self.value["id_count"].as_u64().unwrap();
        let mut args = vec![
            "--uidmap".into(),
            format!("0:{}:{count}", self.value["uid_start"].as_u64().unwrap()),
            "--gidmap".into(),
            format!("0:{}:{count}", self.value["gid_start"].as_u64().unwrap()),
            "--user=0:0".into(),
            "--cap-drop=ALL".into(),
            "--cap-add".into(),
            CAPS.join(","),
            "--cgroupns=private".into(),
            "--device=/dev/fuse".into(),
            "--device=/dev/net/tun".into(),
            "--security-opt=label=disable".into(),
            "--security-opt=seccomp=unconfined".into(),
            "--security-opt=apparmor=unconfined".into(),
            "--security-opt=unmask=/proc/sys".into(),
            "--sysctl=net.ipv4.ip_forward=1".into(),
        ];
        for m in self.value["mounts"].as_array().unwrap() {
            args.extend([
                "--volume".into(),
                format!(
                    "{}:{}:idmap,rprivate,{}",
                    m["source"].as_str().unwrap(),
                    m["destination"].as_str().unwrap(),
                    if m["read_only"] == true { "ro" } else { "rw" }
                ),
            ]);
        }
        args
    }
    pub fn verify_network(&self, scope: &str) -> Result<(), Error> {
        let network = text(&self.value, "network")?;
        if network == "none" {
            return Ok(());
        }
        let out = super::command(30, &["network".into(), "inspect".into(), network.into()])?;
        if !out.status.success() {
            return Err("declared contained network does not exist".into());
        }
        let rows: Vec<Value> = serde_json::from_slice(&out.stdout)?;
        if rows.len() != 1
            || rows[0]["name"] != network
            || rows[0]["labels"][SCOPE_LABEL] != scope
            || rows[0]["driver"] != "bridge"
        {
            return Err("contained network is not an exact scope-owned bridge".into());
        }
        Ok(())
    }
    pub fn verify_unused_ranges(&self) -> Result<(), Error> {
        let inventory = super::command(30, &["ps".into(), "--all".into(), "--format=json".into()])?;
        if !inventory.status.success() {
            return Err("cannot establish existing user namespace allocations".into());
        }
        let rows: Vec<Value> = serde_json::from_slice(&inventory.stdout)?;
        for row in rows {
            let id = row["Id"]
                .as_str()
                .or_else(|| row["ID"].as_str())
                .ok_or("existing container identity unavailable")?;
            let inspect = super::command(30, &["container".into(), "inspect".into(), id.into()])?;
            if !inspect.status.success() {
                return Err("existing user namespace could not be inspected".into());
            }
            let existing: Vec<Value> = serde_json::from_slice(&inspect.stdout)?;
            let c = existing
                .first()
                .filter(|_| existing.len() == 1)
                .ok_or("ambiguous existing container")?;
            if c["Name"] == format!("podmesh-{}", self.uuid)
                || c["Name"] == format!("/podmesh-{}", self.uuid)
            {
                continue;
            }
            if c["HostConfig"]["UsernsMode"] != "private" {
                continue;
            }
            for (key, start) in [("UidMap", "uid_start"), ("GidMap", "gid_start")] {
                let maps = c["HostConfig"]["IDMappings"][key]
                    .as_array()
                    .ok_or("existing private mapping unavailable")?;
                for mapping in maps {
                    let values: Vec<u64> = mapping
                        .as_str()
                        .ok_or("invalid existing mapping")?
                        .split(':')
                        .map(str::parse)
                        .collect::<Result<_, _>>()?;
                    if values.len() != 3 {
                        return Err("unknown existing mapping format".into());
                    }
                    let (a, count, b, length) = (
                        number(&self.value, start)?,
                        number(&self.value, "id_count")?,
                        values[1],
                        values[2],
                    );
                    if a < b.checked_add(length).ok_or("existing mapping overflow")?
                        && b < a + count
                    {
                        return Err("contained range already used by another host container".into());
                    }
                }
            }
        }
        Ok(())
    }
    pub fn verify(&self, c: &Value, binding: &Value) -> Result<Value, Error> {
        if &self.binding() != binding {
            return Err(
                "contained grant/source identity changed; explicit recovery rebind required".into(),
            );
        }
        let host = &c["HostConfig"];
        if host["Privileged"] != false
            || c["Config"]["Labels"][LABEL] != PROFILE
            || c["Config"]["User"] != "0:0"
        {
            return Err(
                "contained universe does not carry its nonprivileged typed envelope".into(),
            );
        }
        for field in ["PidMode", "IpcMode", "UTSMode", "CgroupMode"] {
            if !matches!(host[field].as_str(), Some("" | "private")) {
                return Err("contained namespace is not private".into());
            }
        }
        if host["UsernsMode"] != "private" {
            return Err("contained user namespace unavailable".into());
        }
        for (key, start) in [("UidMap", "uid_start"), ("GidMap", "gid_start")] {
            let expected = json!([format!(
                "0:{}:{}",
                number(&self.value, start)?,
                number(&self.value, "id_count")?
            )]);
            if host["IDMappings"][key] != expected {
                return Err("observed user mappings differ from host grant".into());
            }
        }
        let expected_caps: BTreeSet<String> = CAPS.iter().map(|v| format!("CAP_{v}")).collect();
        let caps = c["BoundingCaps"]
            .as_array()
            .ok_or("contained capability observation unavailable")?;
        let observed_caps: BTreeSet<String> = caps
            .iter()
            .map(|v| v.as_str().map(String::from).ok_or("invalid capability"))
            .collect::<Result<_, _>>()?;
        if observed_caps != expected_caps {
            return Err("observed bounded capabilities differ".into());
        }
        let devices = host["Devices"]
            .as_array()
            .ok_or("contained device observation unavailable")?;
        if devices.len() != 2
            || ["/dev/fuse", "/dev/net/tun"].iter().any(|path| {
                !devices
                    .iter()
                    .any(|d| d["PathOnHost"] == *path && d["PathInContainer"] == *path)
            })
        {
            return Err("observed device envelope differs from bounded nested devices".into());
        }
        let network = text(&self.value, "network")?;
        if network == "none" {
            if host["NetworkMode"] != "none" {
                return Err("contained isolated network differs".into());
            }
        } else if host["NetworkMode"] == "host"
            || c["NetworkSettings"]["Networks"]
                .as_object()
                .map_or(true, |n| n.len() != 1 || !n.contains_key(network))
        {
            return Err("contained network differs from declared scoped network".into());
        }
        let mounts = c["Mounts"]
            .as_array()
            .ok_or("contained mount observation unavailable")?;
        let expected_mounts = self.value["mounts"].as_array().unwrap();
        if mounts.len() != expected_mounts.len() {
            return Err("undeclared contained mount".into());
        }
        for expected in expected_mounts {
            if !mounts.iter().any(|m| {
                m["Type"] == "bind"
                    && m["Source"] == expected["source"]
                    && m["Destination"] == expected["destination"]
                    && m["RW"] == !expected["read_only"].as_bool().unwrap()
                    && matches!(m["Propagation"].as_str(), Some("private" | "rprivate"))
                    && m["Options"]
                        .as_array()
                        .is_some_and(|options| options.iter().any(|o| o == "idmap"))
            }) {
                return Err("observed bind/access/idmap differs from contained grant".into());
            }
        }
        let mut proof = json!({"profile":PROFILE, "configuration_verified":true, "runtime_verified":false, "network":network,
            "uid_start":self.value["uid_start"], "gid_start":self.value["gid_start"], "id_count":self.value["id_count"],
            "volume_recovery":"full-declared-volumes-required; rootfs-only-clone-refused"});
        if c["State"]["Running"] == true || c["State"]["Paused"] == true {
            let pid = c["State"]["Pid"]
                .as_u64()
                .filter(|v| *v > 0)
                .ok_or("contained host PID unavailable")?;
            self.verify_process(pid)?;
            proof["runtime_verified"] = json!(true);
            proof["host_pid"] = json!(pid);
        }
        Ok(proof)
    }
    fn verify_process(&self, pid: u64) -> Result<(), Error> {
        let root = format!("/proc/{pid}");
        let before = fs::read_to_string(format!("{root}/stat"))?;
        for (map, key) in [("uid_map", "uid_start"), ("gid_map", "gid_start")] {
            let data = fs::read_to_string(format!("{root}/{map}"))?;
            let values: Vec<u64> = data
                .split_whitespace()
                .map(str::parse)
                .collect::<Result<_, _>>()?;
            if values
                != [
                    0,
                    number(&self.value, key)?,
                    number(&self.value, "id_count")?,
                ]
            {
                return Err("live kernel mapping differs".into());
            }
        }
        for namespace in ["user", "pid", "ipc", "uts", "net", "cgroup"] {
            if fs::read_link(format!("{root}/ns/{namespace}"))?
                == fs::read_link(format!("/proc/self/ns/{namespace}"))?
            {
                return Err("contained process shares a provider namespace".into());
            }
        }
        let status = fs::read_to_string(format!("{root}/status"))?;
        for (field, key) in [("Uid:", "uid_start"), ("Gid:", "gid_start")] {
            let line = status
                .lines()
                .find(|v| v.starts_with(field))
                .ok_or("live process identity unavailable")?;
            let values: Vec<u64> = line
                .split_whitespace()
                .skip(1)
                .map(str::parse)
                .collect::<Result<_, _>>()?;
            if values.len() != 4
                || values
                    .iter()
                    .any(|v| *v != number(&self.value, key).unwrap())
            {
                return Err("contained root remains privileged or changed identity".into());
            }
        }
        for m in self.value["mounts"].as_array().unwrap() {
            let source = fs::metadata(text(m, "source")?)?;
            let view = fs::metadata(format!("{root}/root{}", text(m, "destination")?))?;
            if view.uid() as u64 != source.uid() as u64 + number(&self.value, "uid_start")?
                || view.gid() as u64 != source.gid() as u64 + number(&self.value, "gid_start")?
                || view.ino() != source.ino()
            {
                return Err("live idmapped volume ownership/identity differs".into());
            }
        }
        // The PID may live for a long time; starttime, not mutable accounting fields, binds it.
        let after = fs::read_to_string(format!("{root}/stat"))?;
        let started = |v: &str| {
            v.rsplit_once(") ")
                .and_then(|(_, rest)| rest.split_whitespace().nth(19))
                .map(String::from)
        };
        if started(&before).is_none() || started(&before) != started(&after) {
            return Err("contained process changed during observation".into());
        }
        Ok(())
    }
}
