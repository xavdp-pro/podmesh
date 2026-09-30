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
    time::Instant,
};

pub const CANONICAL_SHA: &str = "b53e0398bccb1bdc4d562553fd8b3b3fc6c071c64f04d24622e0863cf93b1b36";
pub const CAPS_SHA: &str = "9913177122ebdd1484c1cb34243b051299dee9f02984a31875977f795cae20c9";

pub fn read_private(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(0o400000)
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
        || meta.mode() & 0o077 != 0
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
            .custom_flags(0o400000)
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
            || meta.mode() & 0o077 != 0
        {
            return Err(refusal("fixture_lease_identity_or_mode"));
        }
        file.lock_shared()
            .map_err(|_| refusal("fixture_lease_lock_failed"))?;
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
        let (before, sha) = read_json(path, 1_048_576)?;
        let lease = Lease::shared(Path::new(text(&before, "writer_lock")?))?;
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
        Ok(Self {
            json,
            sha256: sha,
            caps,
            source,
            source_sha256,
            source_path,
            started: Instant::now(),
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
        Caps::parse(&json["resource_caps"])?;
        Ok(())
    }
    pub fn boundary(&self) -> Result<()> {
        if self.started.elapsed().as_secs() >= number(&self.json, "maximum_campaign_seconds")? {
            return Err(refusal("fixture_campaign_time_bound_exceeded"));
        }
        if super::snapshot::bundle_manifest(&self.source_path, &self.caps)? != self.source["bundle"]
        {
            return Err(refusal("source_file_drift"));
        }
        Ok(())
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
