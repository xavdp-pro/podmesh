//! PodMesh Warm-Standby & High-Availability Failover Engine
//! Replaces ha-standby.py with 100% Pure Rust logic.

use crate::crypto::AuthorityKey;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Fencing policy declared for a gated universe or role.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorityPolicy {
    pub universe: String,
    #[serde(rename = "authorityId")]
    pub authority_id: String,
    #[serde(rename = "publicKey")]
    pub public_key: String,
    pub epoch: u64,
    pub holder: Option<String>,
    #[serde(rename = "leaseSeconds")]
    pub lease_seconds: u64,
    #[serde(rename = "marginSeconds")]
    pub margin_seconds: u64,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

/// Cryptographically signed takeover proof.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TakeoverProof {
    pub method: String,
    pub universe: String,
    pub epoch: u64,
    #[serde(rename = "previousHolder")]
    pub previous_holder: Option<String>,
    #[serde(rename = "newHolder")]
    pub new_holder: String,
    #[serde(rename = "barrierNotBefore")]
    pub barrier_not_before: i64,
    pub signature: String,
    #[serde(rename = "signedAt")]
    pub signed_at: String,
}

/// HALedger manages local policies, state and keys in `~/.podmesh-ha`.
pub struct HALedger {
    pub root_dir: PathBuf,
    pub keys_dir: PathBuf,
    pub policies_dir: PathBuf,
}

impl HALedger {
    pub fn new(custom_path: Option<&Path>) -> io::Result<Self> {
        let root_dir = match custom_path {
            Some(p) => p.to_path_buf(),
            None => {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
                PathBuf::from(home).join(".podmesh-ha")
            }
        };

        let keys_dir = root_dir.join("keys");
        let policies_dir = root_dir.join("policies");

        fs::create_dir_all(&keys_dir)?;
        fs::create_dir_all(&policies_dir)?;

        Ok(Self {
            root_dir,
            keys_dir,
            policies_dir,
        })
    }

    /// Declares or initializes a policy for a universe under an authority.
    pub fn declare_policy(
        &self,
        universe: &str,
        authority_id: &str,
        lease: u64,
        margin: u64,
    ) -> io::Result<AuthorityPolicy> {
        let auth_key = AuthorityKey::load_or_create(&self.keys_dir, authority_id)?;
        let policy_path = self.policies_dir.join(format!("{}.json", universe));

        let policy = AuthorityPolicy {
            universe: universe.to_string(),
            authority_id: authority_id.to_string(),
            public_key: auth_key.public_key_hex(),
            epoch: 0,
            holder: None,
            lease_seconds: lease,
            margin_seconds: margin,
            updated_at: Utc::now().to_rfc3339(),
        };

        let serialized = serde_json::to_string_pretty(&policy)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
        fs::write(policy_path, serialized)?;

        Ok(policy)
    }

    /// Loads policy for a given universe.
    pub fn load_policy(&self, universe: &str) -> io::Result<AuthorityPolicy> {
        let policy_path = self.policies_dir.join(format!("{}.json", universe));
        if !policy_path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("Politique HA introuvable pour l'univers '{}'", universe),
            ));
        }

        let content = fs::read_to_string(policy_path)?;
        serde_json::from_str(&content).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
    }

    /// Rotates the epoch and generates a signed takeover proof for the new holder host.
    pub fn rotate_epoch(
        &self,
        universe: &str,
        new_holder: &str,
        barrier_not_before: Option<i64>,
    ) -> Result<TakeoverProof, String> {
        let mut policy = self
            .load_policy(universe)
            .map_err(|e| format!("Erreur chargement politique: {}", e))?;

        let auth_key = AuthorityKey::load_or_create(&self.keys_dir, &policy.authority_id)
            .map_err(|e| format!("Erreur chargement clé autorité: {}", e))?;

        let new_epoch = policy.epoch + 1;
        let now = Utc::now().timestamp();
        let barrier = barrier_not_before.unwrap_or(now + policy.lease_seconds as i64 + policy.margin_seconds as i64);

        let proof_payload = serde_json::json!({
            "method": "rotate",
            "universe": universe,
            "epoch": new_epoch,
            "previous_holder": policy.holder.clone(),
            "new_holder": new_holder,
            "barrier_not_before": barrier
        });

        let signature = auth_key
            .sign_canonical_json(&proof_payload)
            .map_err(|e| format!("Erreur signature Ed25519: {}", e))?;

        let proof = TakeoverProof {
            method: "rotate".to_string(),
            universe: universe.to_string(),
            epoch: new_epoch,
            previous_holder: policy.holder.clone(),
            new_holder: new_holder.to_string(),
            barrier_not_before: barrier,
            signature,
            signed_at: Utc::now().to_rfc3339(),
        };

        // Update local ledger policy
        policy.epoch = new_epoch;
        policy.holder = Some(new_holder.to_string());
        policy.updated_at = Utc::now().to_rfc3339();

        let policy_path = self.policies_dir.join(format!("{}.json", universe));
        let serialized = serde_json::to_string_pretty(&policy).map_err(|e| e.to_string())?;
        fs::write(policy_path, serialized).map_err(|e| e.to_string())?;

        Ok(proof)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ha_ledger_declare_and_rotate() {
        let tmp_dir = std::env::temp_dir().join(format!("podmesh-ha-test-{}", std::process::id()));
        let ledger = HALedger::new(Some(&tmp_dir)).unwrap();

        // 1. Declare policy
        let pol = ledger.declare_policy("univ-db", "auth-master-1", 30, 20).unwrap();
        assert_eq!(pol.epoch, 0);
        assert_eq!(pol.universe, "univ-db");

        // 2. Rotate epoch to host-node-b
        let proof = ledger.rotate_epoch("univ-db", "host-node-b", None).unwrap();
        assert_eq!(proof.epoch, 1);
        assert_eq!(proof.new_holder, "host-node-b");
        assert!(!proof.signature.is_empty());

        // 3. Inspect updated policy
        let reloaded = ledger.load_policy("univ-db").unwrap();
        assert_eq!(reloaded.epoch, 1);
        assert_eq!(reloaded.holder.as_deref(), Some("host-node-b"));

        let _ = fs::remove_dir_all(&tmp_dir);
    }
}
