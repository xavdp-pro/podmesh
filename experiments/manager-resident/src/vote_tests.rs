//! Tests of the signed votes, the signing ledger and readmission (V3-4). Test-only keys, made from a
//! one-byte seed as the node's own tests make them; no real key is made, read or written.
//!
//! Some tests run a child process: this same test binary, re-run on `child_signs_one_vote` with a
//! specification in its environment. A child can crash (abort) at a named point of the ledger's
//! write, or pause after the promise check, which is how a crash between the write and the
//! signature and two signers on one ledger are reproduced with real processes.
use crate::ledger::{self, HostIdentity, Signer, SigningRules};
use crate::quorum::{
    self, testkit, Quorum, POLICY_CHANGE_FIELDS, POLICY_CHANGE_KIND, QUORUM_PROOF_KIND,
    TAKEOVER_FIELDS,
};
use crate::readmission::{self, Scope};
use crate::vote::{self, vote_scope};
use podmesh_manager_ha_lab::{
    durable::{facts_history_sha256, Configuration as Manager},
    Fact, ReplicaConfig, ScopeGrant,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const T0: i64 = 1_700_000_000;
const LIFE: i64 = 300;
/// Past the readmission wait of a ledger created at T0.
const T1: i64 = T0 + LIFE + ledger::CLOCK_SKEW_SECONDS + 1;
const R: &str = "91eeb6bf-5489-405b-b77a-53105b0aff7a";
const R2: &str = "0a6a0a36-5c4b-4a3f-9f3e-7d9c1b2a3e4f";
const X: &str = "5d1c0b8e-3f59-4d0e-9d7a-2a1e7c4b9f10";
const Y: &str = "7e2d1c0b-4a3f-4e5d-8c7b-6a5f4e3d2c1b";
const Z: &str = "3c4d5e6f-7a8b-4c9d-8e0f-1a2b3c4d5e6f";
const BOOT: &str = "0b7f6f1e-6a55-4c1d-8f53-1c2d3e4f5a6b";
const HOST_A: &str = "0123456789abcdef0123456789abcdef";
const HOST_OTHER: &str = "fedcba9876543210fedcba9876543210";
/// The three replicas' keys: (key_id, seed).
const ABC: &[(&str, u8)] = &[("replica-a", 1), ("replica-b", 2), ("replica-c", 3)];
const NODE: &str = "11111111-2222-4333-8444-555555555555";

fn policy(keys: &[(&str, u8)], serial: u64) -> Quorum {
    Quorum::declared("replicas", &testkit::policy(2, keys))
        .unwrap()
        .at_serial(serial)
}

fn rules() -> SigningRules {
    SigningRules {
        max_certificate_life: LIFE,
        lock_wait: Duration::from_secs(5),
    }
}

/// A private directory under the test's temporary directory.
fn private_dir(root: &Path, name: &str) -> PathBuf {
    let dir = root.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

/// Places a test key's seed in `dir` as `<key_id>.key`, a private file.
fn place_key(dir: &Path, key_id: &str, seed: u8) {
    let path = dir.join(format!("{key_id}.key"));
    fs::write(&path, format!("{}\n", quorum::hex(&[seed; 32]))).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn temp_root() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

fn open(dir: &Path, key_id: &str, host: &str, policy: Quorum) -> Signer {
    Signer::open(
        dir,
        key_id,
        HostIdentity::from_machine_id(host).unwrap(),
        policy,
        rules(),
    )
    .unwrap()
}

fn signer_a(dir: &Path) -> Signer {
    open(dir, "replica-a", HOST_A, policy(ABC, 0))
}

fn manager() -> Manager {
    Manager {
        logical_manager_id: "votes-test".into(),
        replicas: ["a", "b", "c"]
            .iter()
            .map(|r| ReplicaConfig {
                replica_id: format!("r-{r}"),
                host_id: format!("h-{r}"),
            })
            .collect(),
        grants: ["a", "b", "c"]
            .iter()
            .map(|r| ScopeGrant {
                scope: vote_scope(&format!("r-{r}")),
                owner_replica_id: format!("r-{r}"),
            })
            .collect(),
    }
}

fn replica_keys() -> BTreeMap<String, String> {
    [
        ("r-a", "replica-a"),
        ("r-b", "replica-b"),
        ("r-c", "replica-c"),
    ]
    .iter()
    .map(|(r, k)| (r.to_string(), k.to_string()))
    .collect()
}

/// The readmission scope of replica r-a: the policy's keys and any retired ones.
fn scope<'a>(
    manager: &'a Manager,
    keys: &'a BTreeMap<String, String>,
    nodes: &'a [String],
    policy: &Quorum,
    retired: &[(&str, u8)],
) -> Scope<'a> {
    let mut known = vote::keys_of(policy);
    for (id, seed) in retired {
        known.insert((*id).to_string(), testkit::key(*seed).verifying_key());
    }
    Scope {
        manager,
        replica_id: "r-a",
        replica_keys: keys,
        known_keys: known,
        policy_keys: policy.keys.iter().map(|k| k.key_id.clone()).collect(),
        retired_keys: retired.iter().map(|(id, _)| (*id).to_string()).collect(),
        nodes,
    }
}

/// A takeover payload under `q`, live from `now` for the certificate life.
fn takeover(q: &Quorum, resource: &str, epoch: i64, holder: &str, now: i64) -> Value {
    json!({"kind": QUORUM_PROOF_KIND, "authority_id": q.authority_id, "policy_digest": q.digest(), "resource": resource,
           "new_epoch": epoch, "previous_epoch": epoch - 1, "new_holder": holder, "previous_holder": null,
           "holder_boot_id": BOOT, "grant_id": format!("g{epoch}"), "method": "lease_barrier",
           "eligible_after": now, "issued_at": now, "expires_at": now + LIFE})
}

/// The same decision with a fresh life: only `issued_at` and `expires_at` move. Anything else, the
/// barrier `eligible_after` included, is another decision.
fn relive(payload: &Value, now: i64) -> Value {
    let mut p = payload.clone();
    p["issued_at"] = json!(now);
    p["expires_at"] = json!(now + LIFE);
    p
}

/// A policy-change payload under `from`, to `to`.
fn change(from: &Quorum, to: &Quorum, resource: &str, now: i64) -> Value {
    json!({"kind": POLICY_CHANGE_KIND, "authority_id": from.authority_id, "policy_digest": from.digest(), "resource": resource,
           "new_policy_digest": to.digest(), "from_serial": from.serial, "new_serial": from.serial + 1,
           "issued_at": now, "expires_at": now + LIFE})
}

/// Evidence files of an empty world: every other replica's store, every other key's ledger and the
/// node's screen, all readable and empty, collected at `at`.
fn write_evidence(
    dir: &Path,
    at: i64,
    stores: &[(&str, Vec<Fact>)],
    ledgers: &[(&str, Value)],
    screen: &[Value],
) {
    let evidence = private_dir(dir, readmission::EVIDENCE_DIR);
    let m = manager();
    for (replica, facts) in stores {
        let content = json!({"history_count": facts.len(), "ordered_facts": facts,
                             "logical_history_sha256": facts_history_sha256(&m, facts).unwrap()});
        write_one(&evidence, "store", replica, at, content);
    }
    for (key, ledger) in ledgers {
        write_one(&evidence, "ledger", key, at, ledger.clone());
    }
    write_one(
        &evidence,
        "screen",
        NODE,
        at,
        json!({"activation_status": screen}),
    );
}

fn write_one(evidence: &Path, input: &str, source: &str, at: i64, content: Value) {
    let file = json!({"form": readmission::EVIDENCE_FORM, "input": input, "source": source, "collected_at": at, "content": content});
    fs::write(
        evidence.join(format!("{input}.{source}.json")),
        serde_json::to_vec(&file).unwrap(),
    )
    .unwrap();
}

/// A peer ledger as evidence: a real ledger file, made by that key's own signer in its own directory.
fn peer_ledger(root: &Path, key_id: &str, seed: u8) -> Value {
    let dir = private_dir(root, &format!("peer-{key_id}"));
    place_key(&dir, key_id, seed);
    let signer = open(&dir, key_id, HOST_OTHER, policy(ABC, 0));
    let ledger = signer.load().or_else(|_| signer.init(T0)).unwrap();
    serde_json::to_value(ledger).unwrap()
}

fn empty_world(root: &Path, dir: &Path, at: i64) {
    let b = peer_ledger(root, "replica-b", 2);
    let c = peer_ledger(root, "replica-c", 3);
    write_evidence(
        dir,
        at,
        &[("r-b", vec![]), ("r-c", vec![])],
        &[("replica-b", b), ("replica-c", c)],
        &[],
    );
}

fn readmit_with(
    signer: &Signer,
    own_store: Vec<Fact>,
    retired: &[(&str, u8)],
    now: i64,
) -> Result<ledger::Admission, readmission::ReadmissionRefusal> {
    let m = manager();
    let keys = replica_keys();
    let nodes = vec![NODE.to_string()];
    let s = scope(&m, &keys, &nodes, signer.policy(), retired);
    let evidence = readmission::gather(&s, signer.dir(), signer.key_id(), Ok(own_store));
    readmission::readmit(signer, &s, evidence, "readmit-test", Some(0), now)
}

/// A ledger for replica-a in `dir`, created at T0 and admitted through the real readmission.
fn admitted_a(root: &Path, dir: &Path) -> Signer {
    place_key(dir, "replica-a", 1);
    let signer = signer_a(dir);
    signer.init(T0).unwrap();
    empty_world(root, dir, T0);
    readmit_with(&signer, vec![], &[], T1).unwrap();
    signer
}

fn code<T: std::fmt::Debug>(result: Result<T, ledger::LedgerRefusal>) -> &'static str {
    match result {
        Ok(_) => "signed",
        Err(e) => e.code,
    }
}

/// A fact of `replica` carrying `votes`, in its vote scope, as its store would hold them.
fn vote_facts(replica: &str, votes: &[Value]) -> Vec<Fact> {
    let topology = manager().topology().unwrap();
    let mut r = topology.instantiate(replica).unwrap();
    votes
        .iter()
        .enumerate()
        .map(|(i, v)| {
            r.observe(
                &vote_scope(replica),
                &format!("vote-{i}"),
                None,
                false,
                &v.to_string(),
            )
            .unwrap()
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Once per (resource, epoch), across restarts.

/// Proves: a key signs one decision per (resource, epoch). The same decision is signed again
/// (a re-issue, with a fresh life too), another holder for that epoch is refused, a lower epoch is
/// refused once a higher one was promised, and all of it holds after the signer is dropped and
/// opened again from its files (a restart). The promise is per resource: another resource's epoch 5
/// is free.
#[test]
fn a_vote_is_signed_once_per_resource_and_epoch_across_restarts() {
    let root = temp_root();
    let dir = private_dir(root.path(), "a");
    let signer = admitted_a(root.path(), &dir);
    let q = policy(ABC, 0);
    let first = signer.sign(&takeover(&q, R, 5, X, T1), &[], T1).unwrap();
    let v = vote::verify_vote(&first, &q).unwrap();
    assert_eq!(
        (v.voter.as_str(), v.ledger_sequence, v.decision.number),
        ("replica-a", 1, 5)
    );
    assert_eq!(
        code(signer.sign(&takeover(&q, R, 5, X, T1), &[], T1)),
        "signed",
        "the same decision again"
    );
    assert_eq!(
        code(signer.sign(&relive(&takeover(&q, R, 5, X, T1), T1 + 10), &[], T1 + 10)),
        "signed",
        "a re-issue with a fresh life"
    );
    assert_eq!(
        code(signer.sign(&takeover(&q, R, 5, X, T1 + 10), &[], T1 + 10)),
        "epoch_already_promised",
        "another barrier is another decision"
    );
    assert_eq!(
        code(signer.sign(&takeover(&q, R, 5, Y, T1), &[], T1)),
        "epoch_already_promised"
    );
    let mut other_grant = takeover(&q, R, 5, X, T1);
    other_grant["grant_id"] = json!("another-grant");
    assert_eq!(
        code(signer.sign(&other_grant, &[], T1)),
        "epoch_already_promised",
        "the same holder under another grant is another decision"
    );
    drop(signer);
    let restarted = signer_a(&dir);
    assert_eq!(
        code(restarted.sign(&takeover(&q, R, 5, Y, T1 + 20), &[], T1 + 20)),
        "epoch_already_promised",
        "after a restart"
    );
    assert_eq!(
        code(restarted.sign(&takeover(&q, R, 6, Y, T1 + 20), &[], T1 + 20)),
        "signed"
    );
    assert_eq!(
        code(restarted.sign(&takeover(&q, R, 5, X, T1 + 20), &[], T1 + 20)),
        "epoch_superseded",
        "below a promised epoch"
    );
    assert_eq!(
        code(restarted.sign(&takeover(&q, R2, 5, Y, T1 + 20), &[], T1 + 20)),
        "signed",
        "another resource"
    );
    let ledger = restarted.load().unwrap();
    assert_eq!(ledger.sequence, 5);
    assert_eq!(
        ledger
            .promise(vote::PromiseKind::Epoch, R, 5)
            .unwrap()
            .holder,
        X
    );
    assert_eq!(
        ledger
            .promise(vote::PromiseKind::Epoch, R2, 5)
            .unwrap()
            .holder,
        Y
    );
}

/// Proves: the signer signs only a payload the node could accept from it, live on its clock and
/// living no longer than the life readmission waits out, under its own policy.
#[test]
fn a_vote_is_signed_only_for_a_live_payload_under_the_replicas_policy() {
    let root = temp_root();
    let dir = private_dir(root.path(), "a");
    let signer = admitted_a(root.path(), &dir);
    let q = policy(ABC, 0);
    let mut long = takeover(&q, R, 5, X, T1);
    long["expires_at"] = json!(T1 + LIFE + 1);
    assert_eq!(code(signer.sign(&long, &[], T1)), "certificate_life");
    assert_eq!(
        code(signer.sign(&takeover(&q, R, 5, X, T1 - LIFE), &[], T1)),
        "certificate_life",
        "expired"
    );
    assert_eq!(
        code(signer.sign(&takeover(&q, R, 5, X, T1 + 31), &[], T1)),
        "certificate_life",
        "issued too far ahead"
    );
    assert_eq!(
        code(signer.sign(&takeover(&policy(ABC, 1), R, 5, X, T1), &[], T1)),
        "policy_mismatch"
    );
    let mut float = takeover(&q, R, 5, X, T1);
    float["note"] = json!(0.5);
    assert_eq!(code(signer.sign(&float, &[], T1)), "payload_invalid");
    let mut skip = takeover(&q, R, 5, X, T1);
    skip["previous_epoch"] = json!(3);
    assert_eq!(code(signer.sign(&skip, &[], T1)), "payload_invalid");
    let mut carries = takeover(&q, R, 5, X, T1);
    carries["signatures"] = json!([]);
    assert_eq!(code(signer.sign(&carries, &[], T1)), "payload_invalid");
    // Nothing refused above was promised.
    assert!(signer
        .load()
        .unwrap()
        .resources
        .get(R)
        .is_none_or(|r| r.epochs.is_empty()));
}

// ---------------------------------------------------------------------------------------------
// The child process: signs one vote, possibly crashing or pausing inside the ledger's write.

const CHILD: &str = "PODMESH_VOTE_TEST_CHILD";

/// Not a test by itself: returns at once unless a parent test runs it as a child process.
#[test]
fn child_signs_one_vote() {
    let Ok(spec) = std::env::var(CHILD) else {
        return;
    };
    let spec: Value = serde_json::from_str(&spec).unwrap();
    let dir = PathBuf::from(spec["dir"].as_str().unwrap());
    let signer = signer_a(&dir);
    let now = spec["now"].as_i64().unwrap();
    let result = signer.sign(&spec["payload"], &[], now);
    fs::write(
        dir.join(format!("outcome-{}", spec["tag"].as_str().unwrap())),
        code(result),
    )
    .unwrap();
}

fn run_child(
    dir: &Path,
    tag: &str,
    payload: &Value,
    now: i64,
    env: &[(&str, String)],
) -> std::process::Child {
    Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "vote_tests::child_signs_one_vote",
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .env(
            CHILD,
            json!({"dir": dir, "tag": tag, "payload": payload, "now": now}).to_string(),
        )
        .env(
            "PODMESH_VOTE_TEST_RELEASE",
            dir.join(format!("released-{tag}")),
        )
        .envs(env.iter().map(|(k, v)| (*k, v.as_str())))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap()
}

fn released(dir: &Path, tag: &str) -> Option<Value> {
    fs::read_to_string(dir.join(format!("released-{tag}")))
        .ok()
        .map(|t| serde_json::from_str(&t).unwrap())
}

// ---------------------------------------------------------------------------------------------
// A crash between the ledger write and the signature.

/// Proves: a crash anywhere in the ledger's write, before the signature, releases no signature, and
/// whatever the disk kept, the key never has signatures for two holders of one epoch.
///
/// For each crash point (after the write and before its fsync; after the fsync and rename and before
/// the directory's fsync; after the directory's fsync and before the signature) a child process signs
/// epoch 7 for X and aborts there. Nothing is released. Then both states the disk can be in after a
/// power loss are tried where the write was not yet durable: the ledger as the process left it, and
/// the ledger as it was before the attempt. In each, the key is asked for epoch 7 for Y and for X, and
/// every signature ever released for epoch 7 is collected: they name one holder. After the
/// directory's fsync the promise is durable and Y is refused.
#[test]
fn a_crash_between_the_ledger_write_and_the_signature_releases_nothing() {
    for point in ["after_write", "after_rename", "after_commit"] {
        let durable = point == "after_commit";
        for power_loss in [false, true] {
            if durable && power_loss {
                continue;
            }
            let root = temp_root();
            let dir = private_dir(root.path(), "a");
            let signer = admitted_a(root.path(), &dir);
            let before = fs::read(signer.ledger_path()).unwrap();
            let q = policy(ABC, 0);
            let status = run_child(
                &dir,
                "crash",
                &takeover(&q, R, 7, X, T1),
                T1,
                &[("PODMESH_VOTE_TEST_CRASH", point.into())],
            )
            .wait()
            .unwrap();
            assert!(!status.success(), "{point}: the child crashed");
            assert!(
                fs::metadata(dir.join("outcome-crash")).is_err(),
                "{point}: the child never returned"
            );
            let mut released_x: Vec<Value> = released(&dir, "crash").into_iter().collect();
            assert!(
                released_x.is_empty(),
                "{point}: a crash before the end released a signature"
            );
            if power_loss {
                fs::write(signer.ledger_path(), &before).unwrap();
            }
            let after = signer_a(&dir);
            let ledger = after
                .load()
                .expect("the ledger is readable after the crash");
            let kept = ledger.promise(vote::PromiseKind::Epoch, R, 7).is_some();
            assert_eq!(
                kept,
                point != "after_write" && !power_loss,
                "{point}, power loss {power_loss}: what the disk kept"
            );
            let y = after.sign(&takeover(&q, R, 7, Y, T1 + 1), &[], T1 + 1);
            assert_eq!(
                code(y.clone()),
                if kept {
                    "epoch_already_promised"
                } else {
                    "signed"
                },
                "{point}, power loss {power_loss}"
            );
            let x = after.sign(&relive(&takeover(&q, R, 7, X, T1), T1 + 2), &[], T1 + 2);
            released_x.extend(y.ok());
            released_x.extend(x.ok());
            let holders: BTreeSet<String> = released_x
                .iter()
                .map(|v| vote::verify_vote(v, &q).unwrap().decision.holder)
                .collect();
            assert_eq!(
                holders.len(),
                1,
                "{point}, power loss {power_loss}: one holder per epoch, got {holders:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// A ledger restored from an older copy: the tripwire.

/// Proves: a ledger restored from an older copy signs again what it forgot while nobody shows it a
/// later vote of its key (the silent revert the tripwire exists for), and trips as soon as one is
/// shown: a vote numbered above the ledger (`ledger_behind_own_votes`), or one numbered since its
/// admission that it does not hold (`own_vote_unknown_to_ledger`). The ledger is then marked
/// unadmitted, durably: shown nothing, it still signs nothing. A vote that claims the key without
/// its signatures fires nothing.
#[test]
fn a_ledger_restored_from_an_older_copy_trips_the_wire() {
    let root = temp_root();
    let dir = private_dir(root.path(), "a");
    let signer = admitted_a(root.path(), &dir);
    let q = policy(ABC, 0);
    let v1 = signer.sign(&takeover(&q, R, 5, X, T1), &[], T1).unwrap();
    let old = fs::read(signer.ledger_path()).unwrap();
    let v2 = signer.sign(&takeover(&q, R, 6, X, T1), &[], T1).unwrap();
    let v3 = signer.sign(&takeover(&q, R, 7, X, T1), &[], T1).unwrap();
    // A forged vote claiming replica-a with a high sequence, signed by replica-b's key.
    let mut forged = v3.clone();
    forged["ledger_sequence"] = json!(99);
    let forged = resign_envelope(&forged, 2);

    // The silent revert: the older copy is put back.
    fs::write(signer.ledger_path(), &old).unwrap();
    let restored = signer_a(&dir);
    assert_eq!(restored.load().unwrap().sequence, 1);
    assert_eq!(code(restored.sign(&takeover(&q, R, 7, Y, T1 + 1), &[v1.clone(), forged.clone()], T1 + 1)), "signed",
               "shown only votes it holds and a forgery, the restored ledger cannot tell: the hole the tripwire closes");

    // Put back again, and shown the peers' facts: the key's votes 2 and 3.
    fs::write(signer.ledger_path(), &old).unwrap();
    let restored = signer_a(&dir);
    assert_eq!(
        code(restored.sign(
            &takeover(&q, R, 7, Y, T1 + 1),
            &[v1.clone(), v2.clone(), v3.clone()],
            T1 + 1
        )),
        "ledger_behind_own_votes"
    );
    let marked = restored.load().unwrap();
    assert!(
        !marked.admitted
            && marked
                .unadmitted_reason
                .as_deref()
                .unwrap()
                .starts_with("tripwire (ledger_behind_own_votes)")
    );
    assert_eq!(
        code(restored.sign(&takeover(&q, R, 8, Y, T1 + 2), &[], T1 + 2)),
        "ledger_unadmitted",
        "durably"
    );

    // The other shape: the restored ledger signed on past the old numbers, so no vote is numbered
    // above it, but a vote numbered since its admission is not in it.
    fs::write(signer.ledger_path(), &old).unwrap();
    let restored = signer_a(&dir);
    restored
        .sign(&takeover(&q, R2, 1, Z, T1 + 3), &[], T1 + 3)
        .unwrap();
    restored
        .sign(&takeover(&q, R2, 2, Z, T1 + 3), &[], T1 + 3)
        .unwrap();
    assert_eq!(restored.load().unwrap().sequence, 3);
    assert_eq!(
        code(restored.sign(
            &takeover(&q, R, 9, Y, T1 + 4),
            std::slice::from_ref(&v2),
            T1 + 4
        )),
        "own_vote_unknown_to_ledger"
    );
    assert!(!restored.load().unwrap().admitted);
}

/// Replaces a vote's envelope signature by one of another key: a forgery of its origin.
fn resign_envelope(vote: &Value, seed: u8) -> Value {
    use ed25519_dalek::Signer as _;
    let mut v = vote.clone();
    v.as_object_mut().unwrap().remove("envelope_signature");
    let message = quorum::canonical_json(&v).unwrap();
    v["envelope_signature"] = json!(quorum::hex(&testkit::key(seed).sign(&message).to_bytes()));
    v
}

/// Replaces a vote's payload signature too, by another key.
fn resign_payload(vote: &Value, seed: u8) -> Value {
    use ed25519_dalek::Signer as _;
    let mut v = vote.clone();
    let message = quorum::canonical_json(&v["payload"]).unwrap();
    v["signature"] = json!(quorum::hex(&testkit::key(seed).sign(&message).to_bytes()));
    resign_envelope(&v, seed)
}

// ---------------------------------------------------------------------------------------------
// A ledger naming another host, another key; missing; unreadable; unadmitted.

/// Proves: the ledger signs only on the host and for the key it was made for, and only admitted. A
/// copy of the whole vote directory (key and ledger) on another host signs nothing
/// (`ledger_foreign_host`), and readmission there refuses too; a ledger put in another key's place
/// signs nothing (`ledger_foreign_key`); a missing ledger, a ledger altered by hand and a new ledger
/// sign nothing, each named.
#[test]
fn a_ledger_naming_another_host_or_key_signs_nothing() {
    let root = temp_root();
    let dir = private_dir(root.path(), "a");
    let signer = admitted_a(root.path(), &dir);
    let q = policy(ABC, 0);
    signer.sign(&takeover(&q, R, 5, X, T1), &[], T1).unwrap();

    let elsewhere = open(&dir, "replica-a", HOST_OTHER, policy(ABC, 0));
    assert_eq!(
        code(elsewhere.sign(&takeover(&q, R, 6, Y, T1), &[], T1)),
        "ledger_foreign_host"
    );
    assert_eq!(
        code(elsewhere.mark_unadmitted("restore", T1)),
        "ledger_foreign_host"
    );
    assert_eq!(
        readmit_with(&elsewhere, vec![], &[], T1 + 10_000)
            .unwrap_err()
            .code,
        "ledger_foreign_host"
    );

    let dir_b = private_dir(root.path(), "b");
    place_key(&dir_b, "replica-b", 2);
    fs::copy(signer.ledger_path(), dir_b.join("replica-b.ledger")).unwrap();
    let b = open(&dir_b, "replica-b", HOST_A, policy(ABC, 0));
    assert_eq!(
        code(b.sign(&takeover(&q, R, 6, Y, T1), &[], T1)),
        "ledger_foreign_key"
    );

    let dir_c = private_dir(root.path(), "c");
    place_key(&dir_c, "replica-c", 3);
    let c = open(&dir_c, "replica-c", HOST_A, policy(ABC, 0));
    assert_eq!(
        code(c.sign(&takeover(&q, R, 6, Y, T1), &[], T1)),
        "ledger_missing"
    );
    c.init(T1).unwrap();
    assert_eq!(
        code(c.sign(&takeover(&q, R, 6, Y, T1), &[], T1)),
        "ledger_unadmitted",
        "a new ledger"
    );
    assert_eq!(code(c.init(T1)), "ledger_exists");
    let mut altered: Value = serde_json::from_slice(&fs::read(c.ledger_path()).unwrap()).unwrap();
    altered["admitted"] = json!(true);
    fs::write(c.ledger_path(), serde_json::to_vec(&altered).unwrap()).unwrap();
    assert_eq!(
        code(c.sign(&takeover(&q, R, 6, Y, T1), &[], T1)),
        "ledger_unreadable",
        "admitted by hand, checksum broken"
    );

    // A key the policy does not name, or names with another public half, opens no signer.
    place_key(&dir_c, "replica-d", 4);
    let refused = Signer::open(
        &dir_c,
        "replica-d",
        HostIdentity::from_machine_id(HOST_A).unwrap(),
        policy(ABC, 0),
        rules(),
    );
    assert_eq!(refused.err().unwrap().code, "key_not_in_policy");
    place_key(&dir_c, "replica-c", 9);
    let refused = Signer::open(
        &dir_c,
        "replica-c",
        HostIdentity::from_machine_id(HOST_A).unwrap(),
        policy(ABC, 0),
        rules(),
    );
    assert_eq!(refused.err().unwrap().code, "key_not_in_policy");
    // The key file must be private.
    place_key(&dir_c, "replica-c", 3);
    fs::set_permissions(
        dir_c.join("replica-c.key"),
        fs::Permissions::from_mode(0o640),
    )
    .unwrap();
    let refused = Signer::open(
        &dir_c,
        "replica-c",
        HostIdentity::from_machine_id(HOST_A).unwrap(),
        policy(ABC, 0),
        rules(),
    );
    assert_eq!(refused.err().unwrap().code, "key_unreadable");
}

// ---------------------------------------------------------------------------------------------
// Two signers on one ledger.

/// Proves: two processes signing on one ledger are serialized by its lock. Both are asked for epoch
/// 9 at once, one for X and one for Y, and each pauses between the promise check and the write,
/// long enough for the other to check too if nothing kept it out. Exactly one signs; the other is
/// refused `epoch_already_promised`. Repeated, whichever wins.
#[test]
fn two_signers_on_one_ledger_are_serialized_by_its_lock() {
    for round in 0..3 {
        let root = temp_root();
        let dir = private_dir(root.path(), "a");
        let _signer = admitted_a(root.path(), &dir);
        let q = policy(ABC, 0);
        let pause = [
            ("PODMESH_VOTE_TEST_PAUSE", "after_check".to_string()),
            ("PODMESH_VOTE_TEST_PAUSE_MS", "400".to_string()),
        ];
        let mut x = run_child(&dir, "x", &takeover(&q, R, 9, X, T1), T1, &pause);
        let mut y = run_child(&dir, "y", &takeover(&q, R, 9, Y, T1), T1, &pause);
        assert!(x.wait().unwrap().success() && y.wait().unwrap().success());
        let outcomes: BTreeSet<String> = ["x", "y"]
            .iter()
            .map(|t| fs::read_to_string(dir.join(format!("outcome-{t}"))).unwrap())
            .collect();
        assert_eq!(
            outcomes,
            ["epoch_already_promised".to_string(), "signed".to_string()].into(),
            "round {round}"
        );
        let signed: Vec<Value> = ["x", "y"]
            .iter()
            .filter_map(|t| released(&dir, t))
            .collect();
        assert_eq!(signed.len(), 1, "round {round}: one signature released");
    }
}

// ---------------------------------------------------------------------------------------------
// Forged origins.

/// Proves: a vote counts only from the key it names, and a relay cannot make one. A vote made by
/// replica-b's key in replica-a's name is refused (`bad_envelope_signature`); so is a real vote of
/// replica-a whose ledger sequence was changed, and one whose payload signature is replaced
/// (`bad_signature`); a vote of a key the policy does not name is refused (`unknown_voter`), and a
/// fact of replica r-b carrying replica-a's vote (`origin_mismatch`). With the forgeries, the
/// assembly still counts one key and makes no certificate.
#[test]
fn a_forged_origin_is_refused() {
    let root = temp_root();
    let dir = private_dir(root.path(), "a");
    let signer = admitted_a(root.path(), &dir);
    let q = policy(ABC, 0);
    let payload = takeover(&q, R, 5, X, T1);
    let genuine = signer.sign(&payload, &[], T1).unwrap();
    let rejected = |v: &Value| {
        vote::verify_vote(v, &q)
            .err()
            .map(|e| e.code)
            .unwrap_or("counted")
    };
    assert_eq!(rejected(&genuine), "counted");

    // replica-b forges a vote in replica-a's name, over the same payload, with its own key.
    let mut in_a_name = genuine.clone();
    in_a_name["voter"] = json!("replica-a");
    let in_a_name = resign_payload(&in_a_name, 2);
    assert_eq!(rejected(&in_a_name), "bad_envelope_signature");
    // A relay changes the sequence number of a real vote.
    let mut renumbered = genuine.clone();
    renumbered["ledger_sequence"] = json!(1000);
    assert_eq!(rejected(&renumbered), "bad_envelope_signature");
    // Only the payload signature replaced, the envelope re-signed by a's key: cannot happen without
    // a's key, but the payload signature is checked on its own too.
    let mut swapped = genuine.clone();
    swapped["signature"] = json!(quorum::hex(&[7; 64]));
    let swapped = resign_envelope(&swapped, 1);
    assert_eq!(rejected(&swapped), "bad_signature");
    // A key the policy does not name.
    let mut outsider = genuine.clone();
    outsider["voter"] = json!("replica-d");
    let outsider = resign_payload(&outsider, 4);
    assert_eq!(rejected(&outsider), "unknown_voter");
    // A vote relayed in another replica's scope does not count as that replica's.
    let facts = vote_facts("r-b", std::slice::from_ref(&genuine));
    assert_eq!(
        vote::vote_of_fact(&facts[0], &replica_keys())
            .unwrap()
            .unwrap_err()
            .code,
        "origin_mismatch"
    );
    let facts = vote_facts("r-a", std::slice::from_ref(&genuine));
    assert!(vote::vote_of_fact(&facts[0], &replica_keys())
        .unwrap()
        .is_ok());

    // Two forgeries and one real vote make one key: no certificate.
    let refusal = vote::assemble(
        &q,
        &payload,
        &[genuine.clone(), in_a_name, outsider, renumbered],
    )
    .unwrap_err();
    assert_eq!(refusal.code, "below_threshold");
    assert_eq!(refusal.rejected.len(), 3);
}

// ---------------------------------------------------------------------------------------------
// Assembly: k-1 votes, and the certificate the node accepts.

fn vote_by(root: &Path, key_id: &str, seed: u8, payload: &Value, now: i64) -> Value {
    let dir = private_dir(root, key_id);
    place_key(&dir, key_id, seed);
    let signer = open(&dir, key_id, HOST_A, policy(ABC, 0));
    if signer.load().is_err() {
        signer.init(T0).unwrap();
        // Admitted by hand for the assembly tests only: the ledger's own rules are tested above.
        admit_by_hand(&signer);
    }
    signer.sign(payload, &[], now).unwrap()
}

/// Proves: fewer than k distinct keys make no certificate. One vote, one voter twice, and two voters
/// on two different payloads (each one short) are all `below_threshold`; two voters on one payload
/// make the certificate, which the node's rules accept.
#[test]
fn k_minus_one_votes_make_no_certificate() {
    let root = temp_root();
    let q = policy(ABC, 0);
    let payload = takeover(&q, R, 5, X, T1);
    let a = vote_by(root.path(), "replica-a", 1, &payload, T1);
    let a_again = vote_by(root.path(), "replica-a", 1, &payload, T1);
    assert_eq!(
        vote::assemble(&q, &payload, std::slice::from_ref(&a))
            .unwrap_err()
            .code,
        "below_threshold"
    );
    assert_eq!(
        vote::assemble(&q, &payload, &[a.clone(), a_again])
            .unwrap_err()
            .code,
        "below_threshold",
        "one key counts once"
    );
    let other = takeover(&q, R, 5, X, T1 + 1);
    let b_other = vote_by(root.path(), "replica-b", 2, &other, T1 + 1);
    assert_eq!(
        vote::assemble(&q, &payload, &[a.clone(), b_other.clone()])
            .unwrap_err()
            .code,
        "below_threshold",
        "votes on another payload do not count"
    );
    assert_eq!(
        vote::assemble(&q, &other, &[a.clone(), b_other])
            .unwrap_err()
            .code,
        "below_threshold"
    );
    let c = vote_by(root.path(), "replica-c", 3, &payload, T1);
    let certificate = vote::assemble(&q, &payload, &[a, c]).unwrap();
    assert_eq!(
        q.verify(&certificate, QUORUM_PROOF_KIND, TAKEOVER_FIELDS)
            .unwrap(),
        ["replica-a", "replica-c"]
    );
}

/// The certificate replica-a and replica-c assemble for this fixed payload (Ed25519 signatures are
/// deterministic, so it is the same bytes at every run). The PodMesh node's own verifier at `5022a7b`
/// (`signing::verify_takeover` under this 2-of-3 policy, digest `965bd61a...`) accepted exactly these
/// bytes, and refused them with one signature removed (`below_threshold`); the run is recorded in
/// EVIDENCE.md.
pub(crate) const NODE_ACCEPTED_CERTIFICATE: &str = r#"{"authority_id":"replicas","eligible_after":1700000600,"expires_at":1700000900,"grant_id":"g12","holder_boot_id":"0b7f6f1e-6a55-4c1d-8f53-1c2d3e4f5a6b","issued_at":1700000600,"kind":"podmesh-takeover-proof/quorum-ed25519","method":"lease_barrier","new_epoch":12,"new_holder":"5d1c0b8e-3f59-4d0e-9d7a-2a1e7c4b9f10","policy_digest":"965bd61aaad93361c01a54635b3f3ee6c40953e0680589f064cf56ca06c19d86","previous_epoch":11,"previous_holder":null,"resource":"91eeb6bf-5489-405b-b77a-53105b0aff7a","signatures":[{"key_id":"replica-a","signature":"PLACEHOLDER_A"},{"key_id":"replica-c","signature":"PLACEHOLDER_C"}]}"#;

fn fixed_payload(q: &Quorum) -> Value {
    json!({"kind": QUORUM_PROOF_KIND, "authority_id": "replicas", "policy_digest": q.digest(), "resource": R,
           "new_epoch": 12, "previous_epoch": 11, "new_holder": X, "previous_holder": null, "holder_boot_id": BOOT,
           "grant_id": "g12", "method": "lease_barrier", "eligible_after": 1_700_000_600, "issued_at": 1_700_000_600, "expires_at": 1_700_000_900})
}

/// Proves: k votes signed by the ledgers assemble into exactly the certificate the node accepts. The
/// policy digest is the node's pinned one; the assembled certificate is byte-for-byte the vector the
/// node's own verifier at `5022a7b` accepted; the port of that verifier accepts it; a vote itself is
/// no certificate (`certificate_kind`, and its `signature` field is `mixed_forms`); and a
/// policy-change certificate assembles and verifies the same way.
#[test]
fn k_votes_assemble_into_the_certificate_the_node_accepts() {
    let root = temp_root();
    let q = policy(ABC, 0);
    assert_eq!(
        q.digest(),
        "965bd61aaad93361c01a54635b3f3ee6c40953e0680589f064cf56ca06c19d86"
    );
    let payload = fixed_payload(&q);
    let now = 1_700_000_600;
    let a = vote_by(root.path(), "replica-a", 1, &payload, now);
    let c = vote_by(root.path(), "replica-c", 3, &payload, now);
    let certificate = vote::assemble(&q, &payload, &[c.clone(), a.clone()]).unwrap();
    let bytes = String::from_utf8(quorum::canonical_json(&certificate).unwrap()).unwrap();
    let expected = NODE_ACCEPTED_CERTIFICATE
        .replace("PLACEHOLDER_A", a["signature"].as_str().unwrap())
        .replace("PLACEHOLDER_C", c["signature"].as_str().unwrap());
    assert_eq!(bytes, expected);
    assert_eq!(
        (
            a["signature"].as_str().unwrap(),
            c["signature"].as_str().unwrap()
        ),
        (NODE_SIGNATURE_A, NODE_SIGNATURE_C),
        "the signatures the node's verifier accepted"
    );
    assert_eq!(
        q.verify(&certificate, QUORUM_PROOF_KIND, TAKEOVER_FIELDS)
            .unwrap(),
        ["replica-a", "replica-c"]
    );
    let mut short = certificate.clone();
    short["signatures"].as_array_mut().unwrap().pop();
    assert_eq!(
        q.verify(&short, QUORUM_PROOF_KIND, TAKEOVER_FIELDS)
            .unwrap_err()
            .code,
        "below_threshold"
    );
    // A vote is not a certificate, whatever it carries.
    assert_eq!(
        q.verify(&a, QUORUM_PROOF_KIND, TAKEOVER_FIELDS)
            .unwrap_err()
            .code,
        "certificate_kind"
    );
    let mut dressed = a.clone();
    dressed["kind"] = json!(QUORUM_PROOF_KIND);
    assert_eq!(
        q.verify(&dressed, QUORUM_PROOF_KIND, TAKEOVER_FIELDS)
            .unwrap_err()
            .code,
        "mixed_forms"
    );

    let next = policy(ABC, 1);
    let change_payload = change(&q, &next, R, now);
    let b = vote_by(root.path(), "replica-b", 2, &change_payload, now);
    let c2 = vote_by(root.path(), "replica-c", 3, &change_payload, now);
    let change_certificate = vote::assemble(&q, &change_payload, &[b, c2]).unwrap();
    assert_eq!(
        q.verify(
            &change_certificate,
            POLICY_CHANGE_KIND,
            POLICY_CHANGE_FIELDS
        )
        .unwrap(),
        ["replica-b", "replica-c"]
    );
}

/// replica-a's and replica-c's signatures over the fixed payload, as the node's verifier accepted them.
const NODE_SIGNATURE_A: &str = "ad813e9941c675456fc8a8ead07b7703d9e002efbab585150d545370067f129ea96b0dcfe0a6ab3b2fbe74ec3536aeaf2fc85671f38aeea2b7b576b530fc1600";
const NODE_SIGNATURE_C: &str = "e3ed43f19d7d8acb38d4e4186b00799f6b8f4c0da6b79943417d6086ff2b8b13ea6b0732707aabd368bf790bac49a208e9c63195c557c239fd07f5c457456301";

// ---------------------------------------------------------------------------------------------
// Policy changes: keyed on (resource, from_serial), and they never reopen an epoch.

/// Proves: a change of the authority set cannot reopen an epoch, and a policy change is promised once
/// per (resource, from_serial).
///
/// (a) The same key across a change: replica-a promised epoch 5 to X under the policy at serial 0; the
/// replica then votes under the same keys at serial 1 (another digest), and epoch 5 to Y is still
/// refused. (b) Two changes from one serial: the second is refused (`serial_already_promised`), and a
/// replica votes for no change away from a serial it is not at (`serial_not_current`). (c) A re-keyed
/// replica: replica-a is retired and replaced by replica-a2 (a new key, a new ledger, a new policy at
/// serial 1). Its readmission reads replica-a's vote for epoch 5 in its store and sets the floor at 5:
/// epoch 5 to Y is refused under the new policy, epoch 6 is signed. (d) Readmission never guesses: the
/// same readmission with the retired key not configured refuses, naming the vote it cannot verify.
#[test]
fn a_policy_change_cannot_reopen_an_epoch() {
    let root = temp_root();
    let dir = private_dir(root.path(), "a");
    let signer = admitted_a(root.path(), &dir);
    let p0 = policy(ABC, 0);
    let p1 = policy(ABC, 1);
    let v5 = signer.sign(&takeover(&p0, R, 5, X, T1), &[], T1).unwrap();
    // (b) one change per (resource, from_serial).
    signer.sign(&change(&p0, &p1, R, T1), &[], T1).unwrap();
    let p1b = || {
        Quorum::declared("replicas", &testkit::policy(3, ABC))
            .unwrap()
            .at_serial(1)
    };
    assert_eq!(
        code(signer.sign(&change(&p0, &p1b(), R, T1), &[], T1)),
        "serial_already_promised"
    );
    assert_eq!(
        code(signer.sign(&change(&p1, &p1b().at_serial(2), R, T1), &[], T1)),
        "policy_mismatch",
        "a change away from a policy it does not vote under"
    );
    // (a) the same key, after the change.
    let after = open(&dir, "replica-a", HOST_A, policy(ABC, 1));
    assert_eq!(
        code(after.sign(&takeover(&p1, R, 5, Y, T1 + 1), &[], T1 + 1)),
        "epoch_already_promised"
    );
    assert_eq!(
        code(after.sign(&takeover(&p1, R, 6, Y, T1 + 1), &[], T1 + 1)),
        "signed"
    );
    assert_eq!(
        code(after.sign(&change(&p0, &p1, R, T1 + 1), &[], T1 + 1)),
        "policy_mismatch"
    );
    let p2_from_1 = Quorum::declared(
        "replicas",
        &testkit::policy(2, &[("replica-a", 1), ("replica-b", 2), ("replica-e", 5)]),
    )
    .unwrap()
    .at_serial(2);
    assert_eq!(
        code(after.sign(&change(&p1, &p2_from_1, R2, T1 + 1), &[], T1 + 1)),
        "signed"
    );
    let behind = open(&dir, "replica-a", HOST_A, policy(ABC, 0));
    assert_eq!(
        code(behind.sign(&change(&p0, &p1b(), R2, T1 + 1), &[], T1 + 1)),
        "serial_superseded",
        "a change from below a serial already promised"
    );

    // (c) re-keyed: replica-a2 replaces replica-a.
    let rekeyed_keys: &[(&str, u8)] = &[("replica-a2", 6), ("replica-b", 2), ("replica-c", 3)];
    let p_new = policy(rekeyed_keys, 1);
    let dir2 = private_dir(root.path(), "a2");
    place_key(&dir2, "replica-a2", 6);
    let a2 = open(&dir2, "replica-a2", HOST_A, policy(rekeyed_keys, 1));
    a2.init(T1).unwrap();
    let own_store = vote_facts("r-a", std::slice::from_ref(&v5));
    let b = peer_ledger(root.path(), "replica-b", 2);
    let c = peer_ledger(root.path(), "replica-c", 3);
    write_evidence(
        &dir2,
        T1,
        &[("r-b", vec![]), ("r-c", vec![])],
        &[("replica-b", b), ("replica-c", c)],
        &[],
    );
    let later = T1 + LIFE + 31;
    // (d) the retired key not configured: the vote cannot be verified, and nothing is guessed.
    let refused = readmit_with_keys(&a2, own_store.clone(), &[], later).unwrap_err();
    assert_eq!(refused.code, "readmission_inputs_unreadable");
    assert!(
        refused
            .unreadable
            .iter()
            .any(|u| u.input == "own_store" && u.reason.contains("replica-a")),
        "{refused:?}"
    );
    let admission = readmit_with_keys(&a2, own_store, &[("replica-a", 1)], later).unwrap();
    assert_eq!(admission.floors_set[R].epoch_floor, 5);
    assert_eq!(
        code(a2.sign(&takeover(&p_new, R, 5, Y, later), &[], later)),
        "epoch_at_or_below_floor"
    );
    assert_eq!(
        code(a2.sign(&takeover(&p_new, R, 6, Y, later), &[], later)),
        "signed"
    );
}

/// Readmission of replica-a2 under the re-keyed policy: its readmission scope names replica-a2 as
/// r-a's key.
fn readmit_with_keys(
    signer: &Signer,
    own_store: Vec<Fact>,
    retired: &[(&str, u8)],
    now: i64,
) -> Result<ledger::Admission, readmission::ReadmissionRefusal> {
    let m = manager();
    let mut keys = replica_keys();
    keys.insert("r-a".into(), signer.key_id().into());
    let nodes = vec![NODE.to_string()];
    let s = scope(&m, &keys, &nodes, signer.policy(), retired);
    let evidence = readmission::gather(&s, signer.dir(), signer.key_id(), Ok(own_store));
    readmission::readmit(signer, &s, evidence, "readmit-rekey", Some(0), now)
}

// ---------------------------------------------------------------------------------------------
// Readmission fails closed.

/// Proves: readmission refuses whenever an input it must read cannot be read, and names each one: a
/// peer's store missing, a peer's store altered after its inspection, a peer's ledger altered, a
/// screen of another host, this replica's own store unreadable, an input collected before the ledger
/// was marked. The ledger stays unadmitted through every refusal. With everything readable it still
/// waits out the longest certificate life since the mark (`readmission_too_early`), then sets the
/// floors above everything seen (the screen's epoch, a peer ledger's promise, a vote in a peer's
/// store) and the sequence above the key's own votes, so the tripwire does not fire on them.
///
/// The scenario is the counter-review's D8b without the omniscient operator: replica-a's host was
/// restored and its ledger lost epoch 1 for B, which only replica-b's host remembers. With replica-b's
/// store and ledger unreadable the old simulator's operator would have admitted a ledger that signs
/// epoch 1 for C; this one refuses.
#[test]
fn readmission_with_an_unreadable_input_refuses() {
    let root = temp_root();
    let dir = private_dir(root.path(), "a");
    let signer = admitted_a(root.path(), &dir);
    let q = policy(ABC, 0);
    let v1 = signer.sign(&takeover(&q, R, 1, X, T1), &[], T1).unwrap();
    // replica-b co-signed epoch 1 for X, in its store and its ledger.
    let dir_b = private_dir(root.path(), "b-live");
    place_key(&dir_b, "replica-b", 2);
    let b = open(&dir_b, "replica-b", HOST_OTHER, policy(ABC, 0));
    b.init(T0).unwrap();
    admit_by_hand(&b);
    let b1 = b.sign(&takeover(&q, R, 1, X, T1), &[], T1).unwrap();
    let b_ledger: Value = serde_json::from_slice(&fs::read(b.ledger_path()).unwrap()).unwrap();
    // The host of replica-a is restored: its ledger comes back empty and is marked unadmitted.
    fs::remove_file(signer.ledger_path()).unwrap();
    signer.init(T1 + 10).unwrap();
    let marked = T1 + 10;
    let after = marked + 1;
    let c_ledger = peer_ledger(root.path(), "replica-c", 3);
    let b_store = vote_facts("r-b", &[b1]);
    let a_store = vote_facts("r-a", &[v1]);
    let screen = vec![
        json!({"this_host_uuid": NODE, "universe_uuid": R, "highest_epoch_seen": 3, "authority_serial": 0}),
    ];
    let full = |dir: &Path| {
        write_evidence(
            dir,
            after,
            &[("r-b", b_store.clone()), ("r-c", vec![])],
            &[
                ("replica-b", b_ledger.clone()),
                ("replica-c", c_ledger.clone()),
            ],
            &screen,
        );
    };
    let evidence_dir = dir.join(readmission::EVIDENCE_DIR);
    let ready = marked + LIFE + ledger::CLOCK_SKEW_SECONDS;
    let refused = |expected_input: &str, expected_source: &str, own: Result<Vec<Fact>, String>| {
        let m = manager();
        let keys = replica_keys();
        let nodes = vec![NODE.to_string()];
        let s = scope(&m, &keys, &nodes, signer.policy(), &[]);
        let evidence = readmission::gather(&s, signer.dir(), signer.key_id(), own);
        let e = readmission::readmit(&signer, &s, evidence, "readmit", Some(0), ready + 100)
            .unwrap_err();
        assert_eq!(e.code, "readmission_inputs_unreadable", "{e}");
        assert!(
            e.unreadable
                .iter()
                .any(|u| u.input == expected_input && u.source == expected_source),
            "{e}"
        );
        assert!(e.alternative.contains("re-key"));
        assert!(!signer.load().unwrap().admitted);
    };
    // replica-b's store missing: the host is down.
    full(&dir);
    fs::remove_file(evidence_dir.join("store.r-b.json")).unwrap();
    refused("store", "r-b", Ok(a_store.clone()));
    // replica-b's store altered after inspection: a fact dropped.
    full(&dir);
    let mut store: Value =
        serde_json::from_slice(&fs::read(evidence_dir.join("store.r-b.json")).unwrap()).unwrap();
    store["content"]["ordered_facts"] = json!([]);
    store["content"]["history_count"] = json!(0);
    fs::write(evidence_dir.join("store.r-b.json"), store.to_string()).unwrap();
    refused("store", "r-b", Ok(a_store.clone()));
    // replica-b's ledger altered: its promise removed.
    full(&dir);
    let mut ledger: Value =
        serde_json::from_slice(&fs::read(evidence_dir.join("ledger.replica-b.json")).unwrap())
            .unwrap();
    ledger["content"]["resources"] = json!({});
    fs::write(
        evidence_dir.join("ledger.replica-b.json"),
        ledger.to_string(),
    )
    .unwrap();
    refused("ledger", "replica-b", Ok(a_store.clone()));
    // A screen of another host under this node's name.
    full(&dir);
    write_one(
        &evidence_dir,
        "screen",
        NODE,
        after,
        json!({"activation_status": [{"this_host_uuid": "another", "universe_uuid": R, "highest_epoch_seen": 3}]}),
    );
    refused("screen", NODE, Ok(a_store.clone()));
    // This replica's own store unreadable.
    full(&dir);
    refused(
        "own_store",
        "r-a",
        Err("the store could not be opened".into()),
    );
    // Collected before the ledger was marked.
    write_evidence(
        &dir,
        marked - 1,
        &[("r-b", b_store.clone()), ("r-c", vec![])],
        &[
            ("replica-b", b_ledger.clone()),
            ("replica-c", c_ledger.clone()),
        ],
        &screen,
    );
    refused("store", "r-c", Ok(a_store.clone()));

    // Everything readable: first too early, then admitted above everything seen.
    full(&dir);
    let early = readmit_with(&signer, a_store.clone(), &[], ready - 1).unwrap_err();
    assert_eq!(
        (early.code, early.retry_at),
        ("readmission_too_early", Some(ready))
    );
    let admission = readmit_with(&signer, a_store.clone(), &[], ready).unwrap();
    assert_eq!(
        admission.floors_set[R].epoch_floor, 3,
        "the screen's epoch is the highest seen"
    );
    assert_eq!(admission.sequence_set, 1, "above the key's own vote");
    assert_eq!(admission.inputs_read.len(), 6);
    let admitted = signer.load().unwrap();
    assert!(admitted.admitted && admitted.admissions.len() == 1);
    let own_vote = a_store[0].value.clone();
    let seen: Vec<Value> = vec![serde_json::from_str(&own_vote).unwrap()];
    assert_eq!(
        code(signer.sign(&takeover(&q, R, 1, Z, ready), &seen, ready)),
        "epoch_at_or_below_floor",
        "epoch 1 for another holder, the D8b case"
    );
    assert_eq!(
        code(signer.sign(&takeover(&q, R, 3, Z, ready), &seen, ready)),
        "epoch_at_or_below_floor"
    );
    assert_eq!(
        code(signer.sign(&takeover(&q, R, 4, Z, ready), &seen, ready)),
        "signed",
        "the old vote does not fire the tripwire"
    );
    assert_eq!(
        readmit_with(&signer, a_store, &[], ready).unwrap_err().code,
        "ledger_admitted"
    );
}

/// Admits a ledger by hand, for a peer whose own readmission is not what a test is about.
fn admit_by_hand(signer: &Signer) {
    let mut ledger = signer.load().unwrap();
    ledger.admitted = true;
    ledger.unadmitted_since = None;
    ledger.unadmitted_reason = None;
    let _lock = signer.lock().unwrap();
    signer.store(&mut ledger).unwrap();
}

/// Proves: the tally of a set of votes makes one certificate per decision and names a conflict only
/// when two certified decisions share an epoch, which kept promises never produce.
#[test]
fn the_tally_finds_no_conflict_when_promises_are_kept() {
    let root = temp_root();
    let q = policy(ABC, 0);
    let p5x = takeover(&q, R, 5, X, T1);
    let p6y = takeover(&q, R, 6, Y, T1);
    let votes = vec![
        vote_by(root.path(), "replica-a", 1, &p5x, T1),
        vote_by(root.path(), "replica-b", 2, &p5x, T1),
        vote_by(root.path(), "replica-b", 2, &p6y, T1),
        vote_by(root.path(), "replica-c", 3, &p6y, T1),
    ];
    let (certificates, conflicts) = vote::tally(&q, &votes);
    assert_eq!(certificates.len(), 2);
    assert!(conflicts.is_empty());
    // A second certificate for epoch 5 needs a key that signs twice: signatures made outside any ledger.
    let p5y = takeover(&q, R, 5, Y, T1);
    let forged: Vec<Value> = [("replica-b", 2_u8), ("replica-c", 3)]
        .iter()
        .map(|(id, seed)| {
            let mut v = votes[0].clone();
            v["voter"] = json!(id);
            v["payload"] = p5y.clone();
            resign_payload(&v, *seed)
        })
        .collect();
    let (_, conflicts) = vote::tally(&q, &[votes, forged].concat());
    assert_eq!(conflicts, [(vote::PromiseKind::Epoch, R.to_string(), 5)]);
}
