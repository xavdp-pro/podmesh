//! Cryptographic Signatures and Verification for PodMesh HA (Ed25519)
//! Signs and validates takeover proofs, epoch rotations, and authority certificates.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Authority keypair manager storing private key securely (0600 permissions).
pub struct AuthorityKey {
    pub authority_id: String,
    pub signing_key: SigningKey,
    pub verifying_key: VerifyingKey,
}

impl AuthorityKey {
    /// Loads an existing Ed25519 authority key or creates a new one at `key_path`.
    pub fn load_or_create(keys_dir: &Path, authority_id: &str) -> io::Result<Self> {
        fs::create_dir_all(keys_dir)?;
        let key_path = keys_dir.join(format!("{}.key", authority_id));

        if key_path.exists() {
            let mut file = File::open(&key_path)?;
            let mut bytes = [0u8; 32];
            file.read_exact(&mut bytes)?;
            let signing_key = SigningKey::from_bytes(&bytes);
            let verifying_key = signing_key.verifying_key();
            Ok(Self {
                authority_id: authority_id.to_string(),
                signing_key,
                verifying_key,
            })
        } else {
            let mut bytes = [0u8; 32];
            let mut urandom = File::open("/dev/urandom")?;
            urandom.read_exact(&mut bytes)?;
            let signing_key = SigningKey::from_bytes(&bytes);
            let verifying_key = signing_key.verifying_key();

            let mut file = File::create(&key_path)?;
            let mut perms = file.metadata()?.permissions();
            perms.set_mode(0o600);
            file.set_permissions(perms)?;

            file.write_all(&signing_key.to_bytes())?;
            file.sync_all()?;

            Ok(Self {
                authority_id: authority_id.to_string(),
                signing_key,
                verifying_key,
            })
        }
    }

    /// Returns public key as lower-case hex string.
    pub fn public_key_hex(&self) -> String {
        hex::encode(self.verifying_key.to_bytes())
    }

    /// Canonicalizes JSON value (sorted keys, compact) and computes Ed25519 signature.
    pub fn sign_canonical_json(&self, value: &serde_json::Value) -> Result<String, String> {
        let canonical_bytes = canonicalize_json(value)?;
        let signature: Signature = self.signing_key.sign(&canonical_bytes);
        Ok(hex::encode(signature.to_bytes()))
    }
}

/// Produces deterministically ordered and compact JSON representation for signing.
pub fn canonicalize_json(value: &serde_json::Value) -> Result<Vec<u8>, String> {
    match value {
        serde_json::Value::Object(map) => {
            let mut btree = std::collections::BTreeMap::new();
            for (k, v) in map {
                btree.insert(k, v);
            }
            serde_json::to_vec(&btree).map_err(|e| e.to_string())
        }
        _ => serde_json::to_vec(value).map_err(|e| e.to_string()),
    }
}

/// Verifies Ed25519 signature over canonical JSON.
pub fn verify_canonical_json(
    public_key_hex: &str,
    value: &serde_json::Value,
    signature_hex: &str,
) -> Result<bool, String> {
    let pub_bytes = hex::decode(public_key_hex).map_err(|e| format!("Clé publique invalide: {}", e))?;
    if pub_bytes.len() != 32 {
        return Err("Taille de clé publique invalide (32 octets attendus)".to_string());
    }
    let mut key_arr = [0u8; 32];
    key_arr.copy_from_slice(&pub_bytes);
    let verifying_key = VerifyingKey::from_bytes(&key_arr).map_err(|e| e.to_string())?;

    let sig_bytes = hex::decode(signature_hex).map_err(|e| format!("Signature invalide: {}", e))?;
    if sig_bytes.len() != 64 {
        return Err("Taille de signature invalide (64 octets attendus)".to_string());
    }
    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(&sig_bytes);
    let signature = Signature::from_bytes(&sig_arr);

    let canonical_bytes = canonicalize_json(value)?;
    Ok(verifying_key.verify_strict(&canonical_bytes, &signature).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ed25519_canonical_sign_verify() {
        let tmp_dir = std::env::temp_dir().join(format!("podmesh-crypto-test-{}", std::process::id()));
        let auth = AuthorityKey::load_or_create(&tmp_dir, "test-authority").unwrap();

        let doc = serde_json::json!({
            "universe": "univ-prod",
            "epoch": 42,
            "holder": "node-alpha",
            "barrier_not_before": 1792416000
        });

        let sig = auth.sign_canonical_json(&doc).unwrap();
        assert_eq!(sig.len(), 128); // 64 bytes in hex

        let valid = verify_canonical_json(&auth.public_key_hex(), &doc, &sig).unwrap();
        assert!(valid);

        let _ = fs::remove_dir_all(&tmp_dir);
    }
}
