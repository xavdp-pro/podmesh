//! PodMesh Decision Follow Engine (Pure Rust)
//! Follows manager replicas' quorum certificates under /etc/podmesh/decision-follow-mandate.
//! 100% Native Pure Rust — Replaces packaging/podmesh-decision-follow Python script.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

pub const DEFAULT_MANDATE: &str = "/etc/podmesh/decision-follow-mandate";
pub const DEFAULT_NODE_SOCKET: &str = "/run/podmesh/api.sock";
pub const QUORUM_PROOF_KIND: &str = "podmesh-takeover-proof/quorum-ed25519";
pub const NODE_TIMEOUT: Duration = Duration::from_secs(30);
pub const PUBLISH_TIMEOUT: Duration = Duration::from_secs(120);
pub const RESIDENT_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone)]
pub struct DecisionFollowMandate {
    pub auth_ref: String,
    pub resources: Vec<String>,
    pub manager_universe: Option<String>,
    pub resident_socket: Option<PathBuf>,
    pub publisher: bool,
}

pub fn is_token(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == ':' || c == '-')
}

pub fn is_uuid(s: &str) -> bool {
    if s.len() != 36 {
        return false;
    }
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 5 || [parts[0].len(), parts[1].len(), parts[2].len(), parts[3].len(), parts[4].len()] != [8, 4, 4, 4, 12] {
        return false;
    }
    s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

pub fn load_mandate(path: &Path) -> Result<DecisionFollowMandate, String> {
    if !path.is_file() {
        return Err(format!("no mandate at {}; nothing is delivered without one", path.display()));
    }

    let content = fs::read_to_string(path).map_err(|e| format!("cannot read mandate: {}", e))?;
    let mut vals = std::collections::HashMap::new();
    let mut resources = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || !line.contains('=') {
            continue;
        }
        let (k, v) = line.split_once('=').unwrap();
        let k = k.trim();
        let v = v.trim();

        if k == "resource" {
            resources.push(v.to_string());
        } else if vals.contains_key(k) {
            return Err(format!("the mandate names {} twice", k));
        } else {
            vals.insert(k.to_string(), v.to_string());
        }
    }

    for k in vals.keys() {
        if !["authorization_ref", "manager_universe", "resident_socket", "publisher"].contains(&k.as_str()) {
            return Err(format!("the mandate carries unknown keys: {}", k));
        }
    }

    let auth_ref = vals.get("authorization_ref").cloned().unwrap_or_default();
    if !is_token(&auth_ref) {
        return Err("the mandate names no usable authorization_ref".to_string());
    }

    if resources.is_empty() {
        return Err("the mandate must name one or more distinct resource UUIDs".to_string());
    }

    let mut seen = HashSet::new();
    for r in &resources {
        if !is_uuid(r) || !seen.insert(r.clone()) {
            return Err("the mandate must name one or more distinct resource UUIDs".to_string());
        }
    }

    let universe = vals.get("manager_universe").cloned();
    let resident_sock = vals.get("resident_socket").cloned();

    if universe.is_some() == resident_sock.is_some() {
        return Err("the mandate names exactly one of manager_universe and resident_socket".to_string());
    }

    if let Some(ref u) = universe {
        if !is_uuid(u) {
            return Err("the mandate's manager_universe is not a UUID".to_string());
        }
    }

    let resident_path = if let Some(ref s) = resident_sock {
        if !s.starts_with('/') || s.split('/').any(|p| p == "..") {
            return Err("the mandate's resident_socket is not an absolute path".to_string());
        }
        Some(PathBuf::from(s))
    } else {
        None
    };

    let publisher_val = vals.get("publisher").map(|s| s.as_str()).unwrap_or("0");
    if publisher_val != "0" && publisher_val != "1" {
        return Err("the mandate's publisher must be 0 or 1".to_string());
    }

    Ok(DecisionFollowMandate {
        auth_ref,
        resources,
        manager_universe: universe,
        resident_socket: resident_path,
        publisher: publisher_val == "1",
    })
}

/// Executes one JSON request on a local Unix domain socket with strict size bounds and timeout.
pub async fn unix_exchange(
    path: &Path,
    payload: &[u8],
    terminate_with_newline: bool,
    timeout_dur: Duration,
) -> Result<Vec<u8>, String> {
    tokio::time::timeout(timeout_dur, async {
        let mut stream = UnixStream::connect(path)
            .await
            .map_err(|e| format!("{}: {}", path.display(), e))?;

        stream.write_all(payload).await.map_err(|e| e.to_string())?;
        if terminate_with_newline {
            stream.write_all(b"\n").await.map_err(|e| e.to_string())?;
        } else {
            stream.shutdown().await.map_err(|e| e.to_string())?;
        }

        let mut buf = Vec::new();
        let mut chunk = [0u8; 65536];
        loop {
            let n = stream.read(&mut chunk).await.map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.len() > 1024 * 1024 {
                return Err(format!("the answer on {} exceeds 1 MiB", path.display()));
            }
        }
        Ok(buf)
    })
    .await
    .map_err(|_| format!("{}: exchange timed out after {:?}", path.display(), timeout_dur))?
}

/// Sends a typed operation to the local PodMesh node daemon.
pub async fn call_node(
    node_socket: &Path,
    operation: &str,
    mut fields: serde_json::Map<String, Value>,
    timeout_dur: Duration,
) -> Result<(Option<Value>, Option<String>), String> {
    fields.insert("operation".to_string(), Value::String(operation.to_string()));
    let payload = serde_json::to_vec(&Value::Object(fields)).map_err(|e| e.to_string())?;

    let raw = unix_exchange(node_socket, &payload, true, timeout_dur).await?;
    let answer: Value = serde_json::from_slice(&raw)
        .map_err(|e| format!("{}: the node's answer is not JSON ({})", operation, e))?;

    let ok = answer
        .get("ok")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| format!("{}: the node's answer has no verdict", operation))?;

    if ok {
        Ok((answer.get("data").cloned(), None))
    } else {
        let err_msg = answer
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error")
            .to_string();
        Ok((None, Some(err_msg)))
    }
}

/// Reads the current decision of a resource from the local manager replica.
pub async fn read_decision(
    node_socket: &Path,
    mandate: &DecisionFollowMandate,
    resource: &str,
) -> Result<Value, String> {
    let reply = if let Some(ref universe) = mandate.manager_universe {
        let mut fields = serde_json::Map::new();
        fields.insert("universe_uuid".to_string(), Value::String(universe.clone()));
        fields.insert("resource".to_string(), Value::String(resource.to_string()));
        fields.insert("authorization_ref".to_string(), Value::String(mandate.auth_ref.clone()));

        let (data, error) = call_node(node_socket, "manager_decision", fields, NODE_TIMEOUT).await?;
        if let Some(err) = error {
            return Err(format!("manager_decision refused: {}", err));
        }
        data.and_then(|d| d.get("resident_reply").cloned()).unwrap_or(Value::Null)
    } else if let Some(ref sock) = mandate.resident_socket {
        let req = json!({
            "operation": "decision_read",
            "resource": resource
        });
        let raw = unix_exchange(sock, req.to_string().as_bytes(), false, RESIDENT_TIMEOUT).await?;
        serde_json::from_slice(&raw).map_err(|e| format!("the resident's answer is not JSON: {}", e))?
    } else {
        return Err("no manager endpoint configured".to_string());
    };

    if !reply.is_object() || reply.get("error").is_some() || reply.get("decision").is_none() {
        return Err(format!("the replica gave no decision: {}", reply));
    }

    let decision = reply.get("decision").unwrap();
    if decision.get("resource").and_then(|r| r.as_str()) != Some(resource) {
        return Err("the replica answered for another resource".to_string());
    }

    if let Some(conflicts) = decision.get("conflicts") {
        if !conflicts.is_null() && (conflicts.is_array() && !conflicts.as_array().unwrap().is_empty()) {
            return Err(format!(
                "the replica reads two certified decisions for epoch(s) {}: a broken promise, which the operator settles; nothing is delivered",
                conflicts
            ));
        }
    }

    Ok(decision.clone())
}

/// Extracts and checks the takeover certificate shape.
pub fn certificate_of(d: &Value, resource: &str) -> Result<Option<Value>, String> {
    let current = match d.get("current") {
        Some(v) if !v.is_null() => v,
        _ => return Ok(None),
    };

    let cert = match current.get("certificate") {
        Some(c) if c.is_object() => c,
        _ => return Ok(None),
    };

    if cert.get("kind").and_then(|v| v.as_str()) != Some(QUORUM_PROOF_KIND)
        || cert.get("resource").and_then(|v| v.as_str()) != Some(resource)
    {
        return Err("the replica's current decision carries no takeover certificate for this resource".to_string());
    }

    if !cert.get("new_epoch").map_or(false, |v| v.is_i64() || v.is_u64())
        || cert.get("new_holder").and_then(|v| v.as_str()).is_none()
    {
        return Err("the certificate names no epoch or holder".to_string());
    }

    if !cert.get("signatures").map_or(false, |v| v.is_array()) {
        return Err("the certificate carries no signatures".to_string());
    }

    Ok(Some(cert.clone()))
}

/// Computes the canonical SHA-256 digest of the certificate excluding the "signatures" key.
pub fn digest_of_certificate(cert: &Value) -> Result<String, String> {
    let obj = cert.as_object().ok_or("certificate is not an object")?;
    let mut payload = serde_json::Map::new();

    for (k, v) in obj {
        if k != "signatures" {
            payload.insert(k.clone(), v.clone());
        }
    }

    // Canonical serialization: keys sorted, compact separators
    let canonical = serde_json::to_string(&Value::Object(payload)).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    Ok(hex::encode(hasher.finalize()))
}

/// Follows a single resource. Returns the JSON report to print on stdout.
pub async fn follow_resource(
    node_socket: &Path,
    mandate: &DecisionFollowMandate,
    resource: &str,
) -> Result<Value, String> {
    let mut report = json!({ "resource": resource });

    let mut status_fields = serde_json::Map::new();
    status_fields.insert("universe_uuid".to_string(), Value::String(resource.to_string()));

    let (status, error) = call_node(node_socket, "activation_status", status_fields, NODE_TIMEOUT).await?;
    if let Some(err) = error {
        return Err(format!("activation_status refused: {}", err));
    }

    let status = status.ok_or("activation_status returned empty data")?;
    if !status.get("authority_quorum").map_or(false, |v| v.as_bool().unwrap_or(false)) {
        return Err("this node's policy for the resource names no quorum of replica keys: a certificate is not delivered to it".to_string());
    }

    let this_host = status
        .get("this_host_uuid")
        .and_then(|v| v.as_str())
        .ok_or("activation_status names no host")?;

    let screen = status
        .get("highest_epoch_seen")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    let d = read_decision(node_socket, mandate, resource).await?;
    let c = certificate_of(&d, resource)?;

    report["screen"] = Value::from(screen);
    if let Some(view_epoch) = d.get("view").and_then(|v| v.get("epoch")) {
        report["decided_epoch"] = view_epoch.clone();
    }

    let c = match c {
        Some(cert) => cert,
        None => {
            report["action"] = Value::String("none: nothing decided yet".to_string());
            return Ok(report);
        }
    };

    let epoch = c.get("new_epoch").and_then(|v| v.as_i64()).unwrap();
    let holder = c.get("new_holder").and_then(|v| v.as_str()).unwrap();
    let digest = digest_of_certificate(&c)?;

    report["epoch"] = Value::from(epoch);
    report["holder"] = Value::String(holder.to_string());
    report["payload_digest"] = Value::String(digest.clone());

    if holder != this_host {
        if epoch <= screen {
            report["action"] = Value::String("none: this node has seen the epoch already".to_string());
            return Ok(report);
        }

        let mut sup_fields = serde_json::Map::new();
        sup_fields.insert("universe_uuid".to_string(), Value::String(resource.to_string()));
        sup_fields.insert("certificate".to_string(), c);
        sup_fields.insert("authorization_ref".to_string(), Value::String(mandate.auth_ref.clone()));
        sup_fields.insert(
            "operation_id".to_string(),
            Value::String(format!("decision-supersede-{}-{}", epoch, &digest[..16])),
        );

        let (_, sup_err) = call_node(node_socket, "activation_supersede", sup_fields, NODE_TIMEOUT).await?;
        report["action"] = Value::String("activation_supersede".to_string());
        if let Some(err) = sup_err {
            report["result"] = Value::String("refused".to_string());
            report["error"] = Value::String(err);
        } else {
            report["result"] = Value::String("delivered".to_string());
        }
        return Ok(report);
    }

    let is_holder = status.get("holder_host_uuid").and_then(|v| v.as_str()) == Some(this_host);
    let is_epoch = status.get("epoch").and_then(|v| v.as_i64()) == Some(epoch);
    let is_live = status.get("live").and_then(|v| v.as_bool()) == Some(true);
    let is_superseded = status.get("superseded").and_then(|v| v.as_bool()).unwrap_or(false);
    let held = is_holder && is_epoch && is_live && !is_superseded;

    if !held {
        if epoch < screen {
            report["action"] = Value::String("none: the certificate is below this node's screen".to_string());
            return Ok(report);
        }

        let generation = status.get("generation").and_then(|v| v.as_i64()).unwrap_or(0);
        let expires_at = status.get("expires_at").and_then(|v| v.as_i64()).unwrap_or(0);
        let situation = format!("{}-{}", generation, expires_at);
        let mut op_id = format!("decision-acquire-{}-{}-{}", epoch, &digest[..16], situation);
        if op_id.len() > 80 {
            op_id.truncate(80);
        }

        let mut acq_fields = serde_json::Map::new();
        acq_fields.insert("universe_uuid".to_string(), Value::String(resource.to_string()));
        acq_fields.insert("certificate".to_string(), c.clone());
        acq_fields.insert("authorization_ref".to_string(), Value::String(mandate.auth_ref.clone()));
        acq_fields.insert("operation_id".to_string(), Value::String(op_id));

        let (_, acq_err) = call_node(node_socket, "activation_acquire", acq_fields, NODE_TIMEOUT).await?;
        report["action"] = Value::String("activation_acquire".to_string());
        if let Some(err) = acq_err {
            report["result"] = Value::String("refused".to_string());
            report["error"] = Value::String(err);
            return Ok(report);
        } else {
            report["result"] = Value::String("delivered".to_string());
        }
    } else {
        report["action"] = Value::String("none: this host holds the lease at the epoch".to_string());
    }

    if mandate.publisher {
        let pub_res = publish_connector(node_socket, mandate, resource, &c, &digest).await;
        report["publisher"] = Value::String(pub_res);
    }

    Ok(report)
}

pub async fn publish_connector(
    node_socket: &Path,
    mandate: &DecisionFollowMandate,
    resource: &str,
    c: &Value,
    digest: &str,
) -> String {
    let mut st_fields = serde_json::Map::new();
    st_fields.insert("resource".to_string(), Value::String(resource.to_string()));

    let (data, error) = match call_node(node_socket, "publisher_status", st_fields, NODE_TIMEOUT).await {
        Ok(res) => res,
        Err(e) => return format!("publisher_status unreadable: {}", e),
    };

    if let Some(err) = error {
        return format!("publisher_status refused: {}", err);
    }

    let data = match data {
        Some(d) => d,
        None => return "publisher_status returned empty".to_string(),
    };

    if data.get("declared").and_then(|v| v.as_bool()) == Some(false) {
        return "none: no publisher declared here".to_string();
    }

    let unit_state = data.get("unit").and_then(|u| u.get("state")).and_then(|v| v.as_str());
    if unit_state == Some("active") || data.get("transition").is_some() {
        return "none: a connector runs or is recorded here".to_string();
    }

    if data.get("publisher_eligible").and_then(|v| v.as_bool()) != Some(true) {
        let reasons: Vec<String> = data
            .get("reasons")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|r| r.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        return format!("none: not eligible ({})", reasons.join("; "));
    }

    let lease = match data.get("lease") {
        Some(l) if l.is_object() => l,
        _ => return "publisher_status names no lease".to_string(),
    };

    let generation = match lease.get("generation").and_then(|v| v.as_i64()) {
        Some(g) => g,
        None => return "publisher_status names no generation".to_string(),
    };

    let acquired = match lease.get("acquired_at").and_then(|v| v.as_i64()) {
        Some(a) => a,
        None => return "publisher_status names no acquired_at".to_string(),
    };

    let op_id = format!("decision-publish-{}-{}-{}", &digest[..16], generation, acquired);
    let mut start_fields = serde_json::Map::new();
    start_fields.insert("resource".to_string(), Value::String(resource.to_string()));
    start_fields.insert("takeover_proof".to_string(), c.clone());
    start_fields.insert("authorization_ref".to_string(), Value::String(mandate.auth_ref.clone()));
    start_fields.insert("operation_id".to_string(), Value::String(op_id));

    match call_node(node_socket, "publisher_start", start_fields, PUBLISH_TIMEOUT).await {
        Ok((_, Some(err))) => format!("publisher_start refused: {}", err),
        Ok((_, None)) => "publisher_start delivered".to_string(),
        Err(e) => format!("publisher_start failed: {}", e),
    }
}

/// Main entry point for podmesh-decision-follow.
pub async fn run_decision_follow(mandate_path: Option<&Path>) -> i32 {
    let default_path = PathBuf::from(
        std::env::var("PODMESH_DECISION_FOLLOW_MANDATE").unwrap_or_else(|_| DEFAULT_MANDATE.to_string()),
    );
    let path = mandate_path.unwrap_or(&default_path);

    let mandate = match load_mandate(path) {
        Ok(m) => m,
        Err(err) => {
            eprintln!("podmesh-decision-follow: {}", err);
            return 3;
        }
    };

    let node_sock = PathBuf::from(
        std::env::var("PODMESH_SOCKET").unwrap_or_else(|_| DEFAULT_NODE_SOCKET.to_string()),
    );

    let mut unreadable_count = 0;
    for resource in &mandate.resources {
        match follow_resource(&node_sock, &mandate, resource).await {
            Ok(rep) => {
                println!("{}", rep);
            }
            Err(e) => {
                unreadable_count += 1;
                let rep = json!({
                    "resource": resource,
                    "action": "none",
                    "unreadable": e
                });
                eprintln!("podmesh-decision-follow: {}: {}; nothing delivered", resource, e);
                println!("{}", rep);
            }
        }
    }

    if unreadable_count > 0 {
        3
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uuid_validation() {
        assert!(is_uuid("91eeb6bf-5489-405b-b77a-53105b0aff7a"));
        assert!(!is_uuid("invalid-uuid"));
        assert!(!is_uuid("91eeb6bf-5489-405b-b77a-53105b0aff7"));
    }

    #[test]
    fn test_certificate_digest() {
        let cert = json!({
            "kind": QUORUM_PROOF_KIND,
            "resource": "91eeb6bf-5489-405b-b77a-53105b0aff7a",
            "new_epoch": 12,
            "new_holder": "host-1",
            "signatures": [{"key_id": "r1", "sig": "abc"}]
        });

        let digest = digest_of_certificate(&cert).unwrap();
        assert!(!digest.is_empty());
        assert_eq!(digest.len(), 64);

        // Same digest when signatures change
        let mut cert2 = cert.clone();
        cert2["signatures"] = json!([{"key_id": "r2", "sig": "different"}]);
        assert_eq!(digest_of_certificate(&cert2).unwrap(), digest);
    }

    #[test]
    fn test_load_mandate_validation() {
        let tmp = std::env::temp_dir().join(format!("test-df-mandate-{}", uuid::Uuid::new_v4()));
        let content = "authorization_ref=test-ref\nmanager_universe=91eeb6bf-5489-405b-b77a-53105b0aff7a\nresource=a1b2c3d4-e5f6-4a1b-8c2d-3e4f5a6b7c8d\npublisher=1\n";
        fs::write(&tmp, content).unwrap();

        let mandate = load_mandate(&tmp).unwrap();
        assert_eq!(mandate.auth_ref, "test-ref");
        assert_eq!(mandate.resources.len(), 1);
        assert!(mandate.publisher);

        let _ = fs::remove_file(tmp);
    }
}

