//! PodMesh Publisher Follow Engine (Pure Rust)
//! Follows publisher connector under /etc/podmesh/publisher-follow-mandate.
//! 100% Native Pure Rust — Replaces packaging/podmesh-publisher-follow Python script.

use crate::decision_follow::{call_node, is_token, is_uuid, NODE_TIMEOUT, PUBLISH_TIMEOUT};
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_MANDATE: &str = "/etc/podmesh/publisher-follow-mandate";
pub const DEFAULT_NODE_SOCKET: &str = "/run/podmesh/api.sock";
pub const DEFAULT_STATE_FILE: &str = "/run/podmesh-publisher-follow/tick-state.json";
pub const UNKNOWN_TICKS: i64 = 3;
pub const BACKOFF_FIRST: i64 = 20;
pub const BACKOFF_CEILING: i64 = 300;

#[derive(Debug, Clone)]
pub struct PublisherFollowMandate {
    pub auth_ref: String,
    pub resource: String,
    pub proof_path: PathBuf,
    pub renew: bool,
    pub not_after: i64,
    pub renew_below: i64,
}

pub fn load_mandate(path: &Path) -> Result<PublisherFollowMandate, String> {
    if !path.is_file() {
        return Err(format!("no mandate at {}; nothing is published without one", path.display()));
    }

    let content = fs::read_to_string(path).map_err(|e| format!("cannot read mandate: {}", e))?;
    let mut vals = std::collections::HashMap::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || !line.contains('=') {
            continue;
        }
        let (k, v) = line.split_once('=').unwrap();
        vals.insert(k.trim().to_string(), v.trim().to_string());
    }

    let auth_ref = vals.get("authorization_ref").cloned().unwrap_or_default();
    let resource = vals.get("resource").cloned().unwrap_or_default();
    let proof_str = vals.get("proof").cloned().unwrap_or_default();
    let renew_str = vals.get("renew").map(|s| s.as_str()).unwrap_or("0");
    let not_after_str = vals.get("not_after").cloned().unwrap_or_default();
    let renew_below_str = vals.get("renew_below").cloned().unwrap_or_default();

    if !is_token(&auth_ref) {
        return Err("the mandate names no usable authorization_ref".to_string());
    }
    if !is_uuid(&resource) {
        return Err("the mandate names no usable resource UUID".to_string());
    }
    if proof_str.is_empty() || !proof_str.starts_with('/') || proof_str.split('/').any(|p| p == "..") {
        return Err("the mandate names no usable absolute proof path".to_string());
    }
    if renew_str != "0" && renew_str != "1" {
        return Err("the mandate's renew must be 0 or 1".to_string());
    }
    let not_after: i64 = not_after_str
        .parse()
        .map_err(|_| "the mandate must carry not_after, the wall-clock second it dies".to_string())?;

    let renew = renew_str == "1";
    let renew_below: i64 = if renew {
        let b: i64 = renew_below_str.parse().map_err(|_| {
            "a mandate that renews must carry renew_below, the seconds of lease left below which it renews"
                .to_string()
        })?;
        if !(1..=86400).contains(&b) {
            return Err("the mandate's renew_below must be from 1 to 86400 seconds".to_string());
        }
        b
    } else {
        0
    };

    Ok(PublisherFollowMandate {
        auth_ref,
        resource,
        proof_path: PathBuf::from(proof_str),
        renew,
        not_after,
        renew_below,
    })
}

/// Shared persistent tick state under /run/podmesh-publisher-follow/tick-state.json
pub struct TickState {
    pub file_path: PathBuf,
    pub resource: String,
    pub mine: serde_json::Map<String, Value>,
}

impl TickState {
    pub fn load(file_path: &Path, resource: &str) -> Self {
        let mine = if let Ok(content) = fs::read_to_string(file_path) {
            if let Ok(Value::Object(whole)) = serde_json::from_str(&content) {
                whole
                    .get(resource)
                    .and_then(|v| v.as_object())
                    .cloned()
                    .unwrap_or_default()
            } else {
                serde_json::Map::new()
            }
        } else {
            serde_json::Map::new()
        };

        TickState {
            file_path: file_path.to_path_buf(),
            resource: resource.to_string(),
            mine,
        }
    }

    pub fn count(&self, key: &str, unreadable: i64) -> i64 {
        self.mine
            .get(key)
            .and_then(|v| v.as_i64())
            .filter(|&v| v >= 0)
            .unwrap_or(unreadable)
    }

    pub fn save(&self) -> bool {
        let parent = self.file_path.parent().unwrap_or(Path::new("."));
        let _ = fs::create_dir_all(parent);

        let lock_path = format!("{}.lock", self.file_path.display());
        let lock_file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .open(&lock_path)
        {
            Ok(f) => f,
            Err(e) => {
                eprintln!("podmesh-publisher-follow: cannot open lock file: {}", e);
                return false;
            }
        };

        use std::os::unix::io::AsRawFd;
        let fd = lock_file.as_raw_fd();
        unsafe {
            if libc::flock(fd, libc::LOCK_EX) != 0 {
                eprintln!("podmesh-publisher-follow: flock failed");
                return false;
            }
        }

        let mut whole: serde_json::Map<String, Value> = if let Ok(content) = fs::read_to_string(&self.file_path) {
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            serde_json::Map::new()
        };

        whole.insert(self.resource.clone(), Value::Object(self.mine.clone()));

        let tmp_path = format!("{}.{}.partial", self.file_path.display(), self.resource);
        let write_ok = (|| -> std::io::Result<()> {
            let data = serde_json::to_vec(&Value::Object(whole))?;
            fs::write(&tmp_path, data)?;
            fs::rename(&tmp_path, &self.file_path)?;
            Ok(())
        })();

        unsafe {
            libc::flock(fd, libc::LOCK_UN);
        }

        if let Err(e) = write_ok {
            eprintln!(
                "podmesh-publisher-follow: the tick's state could not be kept at {}: {}",
                self.file_path.display(),
                e
            );
            false
        } else {
            true
        }
    }
}

pub fn only_the_address_missing(data: &Value) -> bool {
    let gates = match data.get("gates").and_then(|v| v.as_object()) {
        Some(g) => g,
        None => return false,
    };

    let eligible = data.get("publisher_eligible").and_then(|v| v.as_bool()) == Some(true);
    let address_false = gates.get("service_address").and_then(|v| v.as_bool()) == Some(false);
    let others_true = ["lease", "policy", "credential"]
        .iter()
        .all(|&g| gates.get(g).and_then(|v| v.as_bool()) == Some(true));

    !eligible && address_false && others_true
}

pub fn address_unknown(data: &Value) -> bool {
    let gates = match data.get("gates").and_then(|v| v.as_object()) {
        Some(g) => g,
        None => return false,
    };

    let address_null = gates.get("service_address").map_or(false, |v| v.is_null());
    let others_true = ["lease", "policy", "credential"]
        .iter()
        .all(|&g| gates.get(g).and_then(|v| v.as_bool()) == Some(true));

    address_null && others_true
}

pub fn check_running(data: &Value) -> Option<(&'static str, String)> {
    if !["active_manager_mark_read", "origin_ready_at_lease_epoch", "connector_registered"]
        .iter()
        .all(|&k| data.get(k).is_some())
    {
        return None;
    }

    let epoch = data.get("epoch").and_then(|v| v.as_i64());
    let mark = data.get("active_manager_mark_read").and_then(|v| v.as_str());

    if mark == Some("absent") {
        return Some(("mismatch", "no active manager's mark in the carrier".to_string()));
    }

    if mark == Some("present") {
        let mark_epoch = data.get("active_manager_mark_epoch").and_then(|v| v.as_i64());
        if mark_epoch != epoch {
            return Some((
                "mismatch",
                format!(
                    "the active manager's mark names epoch {:?}, the lease is under epoch {:?}",
                    mark_epoch, epoch
                ),
            ));
        }
    }

    if data.get("origin_ready_at_lease_epoch").and_then(|v| v.as_bool()) == Some(false) {
        let readiness = data.get("origin_readiness").unwrap_or(&Value::Null);
        return Some((
            "mismatch",
            format!("the origin is not ready at epoch {:?}: {}", epoch, readiness),
        ));
    }

    if mark != Some("present") {
        let err = data.get("active_manager_mark_error").unwrap_or(&Value::Null);
        return Some(("unknown", format!("the mark could not be read: {}", err)));
    }

    if data.get("origin_ready_at_lease_epoch").map_or(false, |v| v.is_null()) {
        let readiness = data.get("origin_readiness").unwrap_or(&Value::Null);
        return Some(("unknown", format!("the origin's readiness could not be told: {}", readiness)));
    }

    if data.get("connector_registered").map_or(false, |v| v.is_null()) {
        let err = data.get("connector_registration_error").unwrap_or(&Value::Null);
        return Some(("unknown", format!("the current run's journal could not be read: {}", err)));
    }

    if data.get("connector_registered").and_then(|v| v.as_bool()) != Some(true) {
        return Some((
            "unknown",
            "the connector's current run shows no registration with Cloudflare in its journal".to_string(),
        ));
    }

    None
}

pub async fn stop_connector(node_sock: &Path, auth_ref: &str, resource: &str, why: &str) -> Result<(), String> {
    eprintln!("podmesh-publisher-follow: stopping the connector: {}", why);
    let mut fields = serde_json::Map::new();
    fields.insert("resource".to_string(), Value::String(resource.to_string()));
    fields.insert("authorization_ref".to_string(), Value::String(auth_ref.to_string()));
    fields.insert("operation_id".to_string(), Value::String(uuid::Uuid::new_v4().to_string()));

    let (_, err) = call_node(node_sock, "publisher_stop", fields, NODE_TIMEOUT).await?;
    if let Some(e) = err {
        Err(format!("publisher_stop refused: {}", e))
    } else {
        Ok(())
    }
}

pub fn installed_proof(proof_path: &Path, epoch: i64, state: &mut TickState) -> Option<Value> {
    if !proof_path.is_file() {
        return None;
    }

    let raw = match fs::read(proof_path) {
        Ok(b) => b,
        Err(_) => return None,
    };

    let proof: Value = match serde_json::from_slice::<Value>(&raw) {
        Ok(v) if v.is_object() => v,
        Ok(_) | Err(_) => {
            let digest = blake3::hash(&raw).to_hex().to_string();
            if state.mine.get("proof_warned").and_then(|v| v.as_str()) != Some(&digest) {
                eprintln!(
                    "podmesh-publisher-follow: the installed proof at {} cannot be used; starting without it",
                    proof_path.display()
                );
                state.mine.insert("proof_warned".to_string(), Value::String(digest));
            }
            return None;
        }
    };

    state.mine.remove("proof_warned");

    if proof.get("new_epoch").and_then(|v| v.as_i64()) != Some(epoch) {
        eprintln!(
            "podmesh-publisher-follow: installed proof is for epoch {:?}, lease epoch is {}; starting without it, under the same-epoch resume",
            proof.get("new_epoch"),
            epoch
        );
        return None;
    }

    Some(proof)
}

pub async fn run_publisher_follow(mandate_path: Option<&Path>) -> i32 {
    let default_path = PathBuf::from(
        std::env::var("PODMESH_PUBLISHER_FOLLOW_MANDATE").unwrap_or_else(|_| DEFAULT_MANDATE.to_string()),
    );
    let path = mandate_path.unwrap_or(&default_path);

    let mandate = match load_mandate(path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("podmesh-publisher-follow: {}", e);
            return 3;
        }
    };

    let node_sock = PathBuf::from(
        std::env::var("PODMESH_SOCKET").unwrap_or_else(|_| DEFAULT_NODE_SOCKET.to_string()),
    );
    let state_file = PathBuf::from(
        std::env::var("PODMESH_PUBLISHER_FOLLOW_STATE").unwrap_or_else(|_| DEFAULT_STATE_FILE.to_string()),
    );

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let expired = now >= mandate.not_after;

    // 1. Lecture de publisher_status
    let mut status_fields = serde_json::Map::new();
    status_fields.insert("resource".to_string(), Value::String(mandate.resource.clone()));
    status_fields.insert("authorization_ref".to_string(), Value::String(mandate.auth_ref.clone()));
    status_fields.insert("operation_id".to_string(), Value::String(uuid::Uuid::new_v4().to_string()));

    let (status_data, status_err) = match call_node(&node_sock, "publisher_status", status_fields, NODE_TIMEOUT).await {
        Ok(res) => res,
        Err(e) => {
            eprintln!("podmesh-publisher-follow: publisher_status failed: {}", e);
            return 1;
        }
    };

    if let Some(err) = status_err {
        eprintln!("podmesh-publisher-follow: publisher_status refused: {}", err);
        return 1;
    }

    let mut data = status_data.unwrap_or_default();
    if data.get("declared").and_then(|v| v.as_bool()) == Some(false) {
        return 0;
    }

    // 2. Reprise d'adresse de service si seule l'adresse est manquante
    if only_the_address_missing(&data) && !expired {
        let mut resume_fields = serde_json::Map::new();
        resume_fields.insert("exclusive_resource".to_string(), Value::String(mandate.resource.clone()));
        resume_fields.insert("authorization_ref".to_string(), Value::String(mandate.auth_ref.clone()));
        resume_fields.insert("operation_id".to_string(), Value::String(uuid::Uuid::new_v4().to_string()));

        if let Ok((_, None)) = call_node(&node_sock, "network_route_resume", resume_fields, NODE_TIMEOUT).await {
            let mut refetch_fields = serde_json::Map::new();
            refetch_fields.insert("resource".to_string(), Value::String(mandate.resource.clone()));
            refetch_fields.insert("authorization_ref".to_string(), Value::String(mandate.auth_ref.clone()));
            refetch_fields.insert("operation_id".to_string(), Value::String(uuid::Uuid::new_v4().to_string()));
            if let Ok((Some(new_data), None)) = call_node(&node_sock, "publisher_status", refetch_fields, NODE_TIMEOUT).await {
                data = new_data;
            }
        }
    }

    let unit_state = data.get("unit").and_then(|u| u.get("state")).and_then(|v| v.as_str()).unwrap_or("");
    let active = unit_state == "active" || unit_state == "activating";
    let eligible = data.get("publisher_eligible").and_then(|v| v.as_bool()) == Some(true);
    let epoch = data.get("epoch").and_then(|v| v.as_i64()).unwrap_or(0);
    let mut state = TickState::load(&state_file, &mandate.resource);

    // 3. Renouvellement borné (Bounded Renewal)
    if eligible && mandate.renew && !expired {
        let expires_at = data.get("lease").and_then(|l| l.get("expires_at")).and_then(|v| v.as_i64()).unwrap_or(0);
        if expires_at - now <= mandate.renew_below {
            let mut ren_fields = serde_json::Map::new();
            ren_fields.insert("universe_uuid".to_string(), Value::String(mandate.resource.clone()));
            ren_fields.insert("authorization_ref".to_string(), Value::String(mandate.auth_ref.clone()));
            ren_fields.insert("operation_id".to_string(), Value::String(uuid::Uuid::new_v4().to_string()));
            let _ = call_node(&node_sock, "activation_renew", ren_fields, NODE_TIMEOUT).await;
        }
    }

    // 4. Si non-éligible
    if !eligible {
        if address_unknown(&data) {
            if active {
                let n = state.count("unknown_ticks", UNKNOWN_TICKS - 1) + 1;
                if n >= UNKNOWN_TICKS {
                    state.mine.remove("unknown_ticks");
                    state.save();
                    let _ = stop_connector(&node_sock, &mandate.auth_ref, &mandate.resource, &format!("address unknown ({} ticks)", UNKNOWN_TICKS)).await;
                } else {
                    state.mine.insert("unknown_ticks".to_string(), Value::from(n));
                    state.save();
                }
            }
            return 0;
        }

        state.mine.remove("unknown_ticks");
        state.save();
        if active || data.get("transition").is_some() {
            let reasons: Vec<String> = data
                .get("reasons")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|r| r.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default();
            let _ = stop_connector(&node_sock, &mandate.auth_ref, &mandate.resource, &format!("this host is not eligible: {}", reasons.join("; "))).await;
        }
        return 0;
    }

    // 5. Si connecteur actif, vérifier l'état
    if active {
        if let Some((verdict_type, why)) = check_running(&data) {
            if verdict_type == "unknown" {
                let n = state.count("unknown_ticks", UNKNOWN_TICKS - 1) + 1;
                if n >= UNKNOWN_TICKS {
                    state.mine.remove("unknown_ticks");
                    state.save();
                    let _ = stop_connector(&node_sock, &mandate.auth_ref, &mandate.resource, &format!("{} ({} ticks in a row)", why, UNKNOWN_TICKS)).await;
                } else {
                    state.mine.insert("unknown_ticks".to_string(), Value::from(n));
                    if !state.save() {
                        let _ = stop_connector(&node_sock, &mandate.auth_ref, &mandate.resource, &why).await;
                    }
                }
                return 0;
            } else {
                state.mine.remove("unknown_ticks");
                state.save();
                let _ = stop_connector(&node_sock, &mandate.auth_ref, &mandate.resource, &why).await;
                return 0;
            }
        }

        state.mine.remove("unknown_ticks");
        state.save();
        return 0;
    }

    state.mine.remove("unknown_ticks");

    if expired {
        state.save();
        eprintln!(
            "podmesh-publisher-follow: the mandate died at {}; nothing is started under it",
            mandate.not_after
        );
        return 0;
    }

    // 6. Gestion du backoff et démarrage
    if state.mine.get("start_epoch").and_then(|v| v.as_i64()) != Some(epoch) {
        state.mine.remove("start_failures");
        state.mine.remove("next_start_at");
    }

    let next_start = std::cmp::min(
        state.count("next_start_at", now + BACKOFF_CEILING),
        now + BACKOFF_CEILING,
    );

    if now < next_start {
        state.mine.insert("next_start_at".to_string(), Value::from(next_start));
        state.save();
        eprintln!(
            "podmesh-publisher-follow: a start failed {} time(s); next is not before {}",
            state.mine.get("start_failures").unwrap_or(&Value::from(0)),
            next_start
        );
        return 0;
    }

    let proof = installed_proof(&mandate.proof_path, epoch, &mut state);

    let mut start_fields = serde_json::Map::new();
    start_fields.insert("resource".to_string(), Value::String(mandate.resource.clone()));
    start_fields.insert("authorization_ref".to_string(), Value::String(mandate.auth_ref.clone()));
    start_fields.insert("operation_id".to_string(), Value::String(uuid::Uuid::new_v4().to_string()));
    start_fields.insert("previous".to_string(), json!({"source": "publisher-follow"}));
    if let Some(p) = proof {
        start_fields.insert("takeover_proof".to_string(), p);
    }

    match call_node(&node_sock, "publisher_start", start_fields, PUBLISH_TIMEOUT).await {
        Ok((_, None)) => {
            state.mine.remove("start_failures");
            state.mine.remove("next_start_at");
            state.mine.remove("start_epoch");
            state.save();
            0
        }
        Ok((_, Some(err))) => {
            if err.contains("already recorded or active") {
                state.save();
                return 0;
            }
            let failures = std::cmp::min(state.count("start_failures", 16), 16) + 1;
            let next_at = now + std::cmp::min(BACKOFF_CEILING, BACKOFF_FIRST * (1 << (failures - 1)));
            state.mine.insert("start_failures".to_string(), Value::from(failures));
            state.mine.insert("start_epoch".to_string(), Value::from(epoch));
            state.mine.insert("next_start_at".to_string(), Value::from(next_at));
            state.save();
            eprintln!("podmesh-publisher-follow: publisher_start refused: {}", err);
            1
        }
        Err(e) => {
            eprintln!("podmesh-publisher-follow: publisher_start failed: {}", e);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_publisher_mandate() {
        let tmp = std::env::temp_dir().join(format!("test-pf-mandate-{}", uuid::Uuid::new_v4()));
        let content = "authorization_ref=pub-ref\nresource=91eeb6bf-5489-405b-b77a-53105b0aff7a\nproof=/run/proof.json\nrenew=1\nnot_after=1800000000\nrenew_below=300\n";
        fs::write(&tmp, content).unwrap();

        let mandate = load_mandate(&tmp).unwrap();
        assert_eq!(mandate.auth_ref, "pub-ref");
        assert_eq!(mandate.resource, "91eeb6bf-5489-405b-b77a-53105b0aff7a");
        assert!(mandate.renew);
        assert_eq!(mandate.not_after, 1800000000);
        assert_eq!(mandate.renew_below, 300);

        let _ = fs::remove_file(tmp);
    }

    #[test]
    fn test_tick_state_locking_and_counts() {
        let tmp = std::env::temp_dir().join(format!("test-pf-state-{}", uuid::Uuid::new_v4()));
        let mut state = TickState::load(&tmp, "res-1");

        assert_eq!(state.count("unknown_ticks", 0), 0);
        state.mine.insert("unknown_ticks".to_string(), Value::from(2));
        assert!(state.save());

        let reloaded = TickState::load(&tmp, "res-1");
        assert_eq!(reloaded.count("unknown_ticks", 0), 2);

        let _ = fs::remove_file(&tmp);
        let _ = fs::remove_file(format!("{}.lock", tmp.display()));
    }
}

