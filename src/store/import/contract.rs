//! Explicit private synthetic fixture contracts and cooperative cleanup/source leases.
//! These are operator attestations and OS advisory locks, not a hostile-operator sandbox.
use super::{canonical, caps::Caps, refusal};
use crate::store::Result;
use serde_json::{json, Value as Json};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub const CANONICAL_SHA: &str = "b53e0398bccb1bdc4d562553fd8b3b3fc6c071c64f04d24622e0863cf93b1b36";
pub const CAPS_SHA: &str = "9913177122ebdd1484c1cb34243b051299dee9f02984a31875977f795cae20c9";

pub fn read_private(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(0o400000 | 0o4000)
        .open(path)
        .map_err(|_| refusal("private_input_open_failed"))?;
    let meta = file
        .metadata()
        .map_err(|_| refusal("private_input_stat_failed"))?;
    let named = fs::symlink_metadata(path).map_err(|_| refusal("private_input_stat_failed"))?;
    if !meta.is_file()
        || !named.is_file()
        || meta.dev() != named.dev()
        || meta.ino() != named.ino()
        || !matches!(meta.mode() & 0o7777, 0o600 | 0o400)
        || meta.len() > limit
    {
        return Err(refusal("private_input_identity_mode_or_limit"));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| refusal("private_input_read_failed"))?;
    if bytes.len() as u64 != meta.len() {
        return Err(refusal("private_input_drift_or_limit"));
    }
    Ok(bytes)
}

/// Same descriptor, finite streaming hash; no execution of the administrator's SQL dump.
pub fn hash_private_stream(path:&Path,expected:u64,limit:u64,boundary:&mut dyn FnMut()->Result<()>)->Result<String> {
    use sha2::{Digest,Sha256};
    boundary()?;
    if expected==0 || expected>limit{return Err(refusal("dump_input_size_refused"));}
    let mut file=OpenOptions::new().read(true).custom_flags(0o400000|0o4000).open(path).map_err(|_|refusal("dump_input_open_failed"))?;
    let meta=file.metadata().map_err(|_|refusal("dump_input_stat_failed"))?;
    let named=fs::symlink_metadata(path).map_err(|_|refusal("dump_input_stat_failed"))?;
    if !meta.is_file() || !named.is_file() || meta.dev()!=named.dev() || meta.ino()!=named.ino() || !matches!(meta.mode()&0o7777,0o600|0o400) || meta.len()!=expected{return Err(refusal("dump_input_identity_mode_or_size"));}
    let mut hash=Sha256::new();let mut buffer=[0u8;65536];let mut total=0u64;
    loop {boundary()?;let n=file.read(&mut buffer).map_err(|_|refusal("dump_input_read_failed"))?;boundary()?;if n==0{break;}
        total=total.checked_add(n as u64).ok_or_else(||refusal("resource_counter_overflow"))?;
        if total>expected || total>limit{return Err(refusal("dump_input_drift_or_limit"));}hash.update(&buffer[..n]);}
    let after=file.metadata().map_err(|_|refusal("dump_input_stat_failed"))?;
    if total!=expected || after.len()!=meta.len() || after.mtime()!=meta.mtime() || after.mtime_nsec()!=meta.mtime_nsec(){return Err(refusal("dump_input_drift_or_limit"));}
    boundary()?;Ok(format!("{:x}",hash.finalize()))
}

/// Hash the exact bytes parsed once, including A-issued resource/administrator records.
pub fn pinned_json(spec:&Json)->Result<Json> {
    let (value,sha)=read_json(Path::new(text(spec,"path")?),8_388_608)?;
    if sha!=text(spec,"sha256")?{return Err(refusal("fixture_pinned_json_checksum_mismatch"));}
    Ok(value)
}

pub fn read_json(path: &Path, limit: u64) -> Result<(Json, String)> {
    let bytes = read_private(path, limit)?;
    let json = serde_json::from_slice(&bytes).map_err(|_| refusal("private_json_invalid"))?;
    Ok((json, canonical::hash(&bytes)))
}

pub fn text<'a>(json: &'a Json, key: &str) -> Result<&'a str> {
    json[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| refusal("fixture_required_field_missing"))
}
pub fn number(json: &Json, key: &str) -> Result<u64> {
    json[key]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or_else(|| refusal("fixture_required_bound_missing"))
}

pub fn check_binary_pin(json: &Json) -> Result<()> {
    use sha2::{Digest, Sha256};
    let commit = text(json, "source_commit")?;
    let pinned = text(json, "binary_sha256")?;
    if commit.len() != 40
        || !commit.bytes().all(|b| b.is_ascii_hexdigit())
        || pinned.len() != 64
        || !pinned.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(refusal("fixture_source_binary_provenance_missing"));
    }
    let path = std::env::current_exe().map_err(|_| refusal("fixture_binary_path_unavailable"))?;
    let mut file = File::open(path).map_err(|_| refusal("fixture_binary_read_failed"))?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|_| refusal("fixture_binary_read_failed"))?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    if format!("{:x}", hash.finalize()) != pinned {
        return Err(refusal("fixture_binary_pin_mismatch"));
    }
    // Source commit is A's attestation binding that executable's build receipt, not a runtime
    // claim that a mutable worktree or a hostile operator is constrained by this contract.
    Ok(())
}

pub struct Lease {
    _file: File,
}
impl Lease {
    pub fn shared(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(0o400000 | 0o4000)
            .open(path)
            .map_err(|_| refusal("fixture_lease_open_failed"))?;
        let meta = file
            .metadata()
            .map_err(|_| refusal("fixture_lease_stat_failed"))?;
        let named = fs::symlink_metadata(path).map_err(|_| refusal("fixture_lease_stat_failed"))?;
        if !meta.is_file()
            || !named.is_file()
            || meta.dev() != named.dev()
            || meta.ino() != named.ino()
            || !matches!(meta.mode() & 0o7777, 0o600 | 0o400)
        {
            return Err(refusal("fixture_lease_identity_or_mode"));
        }
        file.try_lock_shared().map_err(|err| match err {
            std::fs::TryLockError::WouldBlock => refusal("fixture_lease_contended"),
            _ => refusal("fixture_lease_lock_failed"),
        })?;
        Ok(Self { _file: file })
    }
}

pub struct Contract {
    pub json: Json,
    pub sha256: String,
    pub caps: Caps,
    pub source: Json,
    pub source_sha256: String,
    pub source_path: PathBuf,
    pub started: Instant,
    _target_lease: Lease,
    _source_lease: Lease,
}
impl Contract {
    pub fn open(path: &Path, roles: &[&str]) -> Result<Self> {
        let started = Instant::now();
        let (before, sha) = read_json(path, 1_048_576)?;
        Self::validate(&before, roles)?;
        let budget = Duration::from_secs(number(&before, "maximum_campaign_seconds")?);
        if started.elapsed() >= budget {
            return Err(refusal("fixture_campaign_time_bound_exceeded"));
        }
        let lease = Lease::shared(Path::new(text(&before, "writer_lock")?))?;
        if started.elapsed() >= budget {
            return Err(refusal("fixture_campaign_time_bound_exceeded"));
        }
        let (json, locked_sha) = read_json(path, 1_048_576)?;
        if locked_sha != sha {
            return Err(refusal("fixture_contract_changed_under_lease"));
        }
        Self::validate(&json, roles)?;
        let source_manifest_path = Path::new(text(&json, "source_manifest_path")?);
        let (source, source_sha256) = read_json(source_manifest_path, 1_048_576)?;
        if source_sha256 != text(&json, "source_manifest_sha256")? {
            return Err(refusal("fixture_source_manifest_sha_mismatch"));
        }
        if source["synthetic_only"] != true
            || source["customer_data"] != false
            || source["real_keys_or_credentials"] != false
            || source["all_owners_and_maintenance_quiesced"] != true
            || source["generator"] != "podmesh-c02-synthetic/1"
        {
            return Err(refusal("synthetic_source_contract_required"));
        }
        let source_path = PathBuf::from(text(&source, "source_path")?);
        let source_lease = Lease::shared(Path::new(text(&source, "source_lock")?))?;
        let caps = Caps::parse(&json["resource_caps"])?;
        if super::snapshot::bundle_manifest(&source_path, &caps)? != source["bundle"] {
            return Err(refusal("source_bundle_manifest_mismatch"));
        }
        if started.elapsed() >= budget {
            return Err(refusal("fixture_campaign_time_bound_exceeded"));
        }
        Ok(Self {
            json,
            sha256: sha,
            caps,
            source,
            source_sha256,
            source_path,
            started,
            _target_lease: lease,
            _source_lease: source_lease,
        })
    }
    pub fn validate(json: &Json, roles: &[&str]) -> Result<()> {
        if json["schema"] != "podmesh-c02-fixture/1"
            || json["status"] != "ready"
            || json["server_execution_authorized"] != true
            || !roles.contains(&text(json, "role")?)
        {
            return Err(refusal("fixture_execution_not_authorized"));
        }
        if json["canonical_contract_sha256"] != CANONICAL_SHA
            || json["resource_caps_contract_sha256"] != CAPS_SHA
            || json["target_resource_identity"]
                .as_object()
                .filter(|o| !o.is_empty())
                .is_none()
        {
            return Err(refusal("fixture_contract_pins_missing"));
        }
        for key in [
            "database",
            "dsn",
            "server_version",
            "current_user",
            "migration_id",
            "source_manifest_path",
            "source_manifest_sha256",
            "writer_lock",
            "database_charset",
            "database_collation",
        ] {
            text(json, key)?;
        }
        for key in [
            "connect_timeout_ms",
            "lock_wait_timeout_seconds",
            "statement_timeout_ms",
            "maximum_campaign_seconds",
        ] {
            number(json, key)?;
        }
        for key in ["container_id", "volume_name", "image_id"] {
            text(&json["target_resource_identity"], key)?;
        }
        let id = text(json, "migration_id")?;
        if id.len() > 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(refusal("fixture_migration_id_domain_exceeded"));
        }
        for key in ["source_manifest_sha256", "binary_sha256"] {
            let sha = text(json, key)?;
            if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(refusal("fixture_checksum_invalid"));
            }
        }
        let source_commit = text(json, "source_commit")?;
        if source_commit.len() != 40 || !source_commit.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(refusal("fixture_source_commit_invalid"));
        }
        if json["role"] == "capability" {
            text(json, "admission_probe_directory")?;
        }
        if matches!(json["role"].as_str(),Some("seeded_restore"|"restored_copy")) {
            text(&json["dump_restore_receipt"],"path")?;text(&json["dump_restore_receipt"],"sha256")?;
            let helper=text(json,"dump_restore_helper_sha256")?;
            if helper.len()!=64 || !helper.bytes().all(|b|b.is_ascii_hexdigit()){return Err(refusal("fixture_dump_helper_pin_invalid"));}
        }
        Caps::parse(&json["resource_caps"])?;
        Ok(())
    }
    pub fn clock_boundary(&self) -> Result<()> {
        if self.started.elapsed().as_secs() >= number(&self.json, "maximum_campaign_seconds")? {
            return Err(refusal("fixture_campaign_time_bound_exceeded"));
        }
        Ok(())
    }
    pub fn boundary(&self) -> Result<()> {
        self.clock_boundary()?;
        if super::snapshot::bundle_manifest(&self.source_path, &self.caps)? != self.source["bundle"]
        {
            return Err(refusal("source_file_drift"));
        }
        self.clock_boundary()
    }
    pub fn target_identity(&self) -> Result<String> {
        Ok(canonical::hash(
            &serde_json::to_vec(&self.json["target_resource_identity"])
                .map_err(|_| refusal("fixture_identity_invalid"))?,
        ))
    }
    pub fn receipt(&self, key: &str, format: &str) -> Result<Json> {
        let spec = &self.json[key];
        let (receipt, sha) = read_json(Path::new(text(spec, "path")?), 8_388_608)?;
        if sha != text(spec, "sha256")?
            || receipt["format"] != format
            || receipt["status"] != "PASS"
        {
            return Err(refusal("fixture_prerequisite_receipt_invalid"));
        }
        Ok(receipt)
    }
    pub fn event(&self, point: &str) -> Result<()> {
        self.boundary()?;
        if self.json["killpoint"].as_str() == Some(point) {
            let event = Path::new(text(&self.json, "kill_event_path")?);
            super::snapshot::private_json(
                event,
                &json!({"format":"podmesh-c02-killpoint/1","point":point,"pid":std::process::id(),"contract_sha256":self.sha256,"migration_id":self.json["migration_id"],"status":"WAITING_FOR_EXTERNAL_SIGKILL"}),
            )?;
            // Only the private campaign controller kills this process; a bounded wait failure
            // is not reported as a SIGKILL proof. Cleanup remains blocked by the shared lease.
            loop {
                std::thread::sleep(std::time::Duration::from_millis(100));
                self.boundary()?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absent_closed_wrong_role_or_unpinned_contract_refuses_before_connect() {
        for json in [
            json!(null),
            json!({"schema":"podmesh-c02-fixture/1","status":"closed","server_execution_authorized":true}),
            json!({"schema":"podmesh-c02-fixture/1","status":"ready","server_execution_authorized":false}),
            json!({"schema":"podmesh-c02-fixture/1","status":"ready","server_execution_authorized":true,"role":"operational"}),
        ] {
            assert!(Contract::validate(&json, &["copy"]).is_err());
        }
        assert!(Contract::open(Path::new("/nonexistent-podmesh-C02-contract"), &["copy"]).is_err());
    }
    #[test]
    fn contended_cleanup_lease_refuses_without_waiting_or_connecting() {
        let path = std::env::temp_dir().join(format!(
            "podmesh-c02-contention-{}-{}",
            std::process::id(),
            crate::now()
        ));
        super::super::snapshot::create_file(&path).unwrap();
        let owner = File::open(&path).unwrap();
        owner.try_lock().unwrap();
        assert_eq!(
            Lease::shared(&path).err().unwrap().message,
            "fixture_lease_contended"
        );
        drop(owner);
        Lease::shared(&path).unwrap();
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn fifo_inputs_and_leases_are_nonblocking_regular_file_refusals() {
        let path = std::env::temp_dir().join(format!(
            "podmesh-c02-fifo-{}-{}",
            std::process::id(),
            crate::now()
        ));
        assert!(std::process::Command::new("mkfifo")
            .args(["-m", "600"])
            .arg(&path)
            .status()
            .unwrap()
            .success());
        for lease in [false, true] {
            let (send, recv) = std::sync::mpsc::channel();
            let named = path.clone();
            let worker = std::thread::spawn(move || {
                let err = if lease {
                    Lease::shared(&named).err().unwrap()
                } else {
                    read_private(&named, 1024).err().unwrap()
                };
                send.send(err.message).unwrap();
            });
            let result = recv.recv_timeout(std::time::Duration::from_secs(3));
            if result.is_err() {
                let _rescue = OpenOptions::new()
                    .write(true)
                    .custom_flags(0o4000)
                    .open(&path);
                panic!("FIFO input blocked before regularity refusal");
            }
            assert!(result.unwrap().contains(if lease {
                "identity_or_mode"
            } else {
                "identity_mode_or_limit"
            }));
            worker.join().unwrap();
        }
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn private_control_inputs_require_exact_600_or_400_not_executable_modes() {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!(
            "podmesh-c02-mode-{}-{}",
            std::process::id(),
            crate::now()
        ));
        super::super::snapshot::private_json(&path, &json!({"synthetic":true})).unwrap();
        read_private(&path, 1024).unwrap();
        Lease::shared(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
        read_private(&path, 1024).unwrap();
        Lease::shared(&path).unwrap();
        for mode in [0o700, 0o744, 0o644] {
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            assert!(read_private(&path, 1024).is_err());
            assert!(Lease::shared(&path).is_err());
        }
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn dump_stream_refuses_size_nonregular_and_observes_budget_without_execution() {
        use std::io::Write;
        let path=std::env::temp_dir().join(format!("podmesh-dump-stream-{}-{}",std::process::id(),crate::now()));
        let bytes=vec![42u8;131072];super::super::snapshot::create_file(&path).unwrap().write_all(&bytes).unwrap();
        assert_eq!(hash_private_stream(&path,bytes.len() as u64,67108864,&mut ||Ok(())).unwrap(),canonical::hash(&bytes));
        assert!(hash_private_stream(&path,bytes.len() as u64+1,67108864,&mut ||Ok(())).is_err());
        assert!(hash_private_stream(&path,67108865,67108864,&mut ||Ok(())).is_err());
        let mut calls=0;assert_eq!(hash_private_stream(&path,bytes.len() as u64,67108864,&mut ||{calls+=1;if calls==4{Err(refusal("injected_dump_budget"))}else{Ok(())}}).unwrap_err().message,"injected_dump_budget");
        fs::remove_file(&path).unwrap();
        assert!(std::process::Command::new("mkfifo").args(["-m","600"]).arg(&path).status().unwrap().success());
        assert_eq!(hash_private_stream(&path,1,67108864,&mut ||Ok(())).unwrap_err().message,"dump_input_identity_mode_or_size");fs::remove_file(path).unwrap();
    }

    #[test]
    fn cleanup_exclusive_lock_cannot_be_taken_during_reader_lease() {
        let path = std::env::temp_dir().join(format!(
            "podmesh-c02-lease-{}-{}",
            std::process::id(),
            crate::now()
        ));
        super::super::snapshot::create_file(&path).unwrap();
        let lease = Lease::shared(&path).unwrap();
        let owner = File::open(&path).unwrap();
        assert!(owner.try_lock().is_err());
        drop(lease);
        owner.try_lock().unwrap();
        fs::remove_file(path).unwrap();
    }
}
