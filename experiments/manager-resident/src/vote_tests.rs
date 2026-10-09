//! Tests of the signed votes, the signing ledger and readmission (V3-4). Test-only keys, made from a
//! one-byte seed as the node's own tests make them; no real key is made, read or written.
//!
//! Some tests run a child process: this same test binary, re-run on `child_signs_one_vote` with a
//! specification in its environment. A child can crash (abort) at a named point of the ledger's
//! write, or pause between reading the ledger and checking the promise: that is how a crash between
//! the write and the signature, and two signers on one ledger, are reproduced with real processes.
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
const T1: i64 = T0 + LIFE + 60 + 1;
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
    let mut signer = Signer::open(
        dir,
        key_id,
        HostIdentity::from_machine_id(host).unwrap(),
        policy,
        rules(),
    )
    .unwrap();
    // These suites test what the ledger and the policy decide, not the hypervisor witness. A signer
    // that says nothing about the witness now refuses to sign, so each one says here, once, that it
    // is deliberately doing without. The suites that do test the witness watch one instead.
    signer
        .waive_generation("test signer: these suites do not exercise the generation witness")
        .unwrap();
    signer
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
    let evidence = evidence_dir_of(dir);
    private(&evidence);
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

/// The operator's evidence directory for the replica whose vote directory is `vote_dir`: a sibling,
/// outside it.
fn evidence_dir_of(vote_dir: &Path) -> PathBuf {
    vote_dir.with_file_name(format!(
        "{}-evidence",
        vote_dir.file_name().unwrap().to_str().unwrap()
    ))
}

fn private(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
}

/// What the operator states: the SHA-256 of every evidence file in `dir`, as it is now.
fn stated_digests(dir: &Path) -> BTreeMap<String, String> {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .map(|e| e.unwrap())
                .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                .map(|e| {
                    (
                        e.file_name().into_string().unwrap(),
                        quorum::sha256_hex(&fs::read(e.path()).unwrap()),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Gathers a readmission's evidence from the operator's evidence directory of `signer`, with the
/// digests stated as `stated`, or as the files are now.
fn gather_for(
    s: &Scope<'_>,
    signer: &Signer,
    stated: Option<&BTreeMap<String, String>>,
    own: Result<Vec<Fact>, String>,
) -> readmission::Evidence {
    let dir = evidence_dir_of(signer.dir());
    let now_stated = stated_digests(&dir);
    readmission::gather(
        s,
        &readmission::EvidenceSource {
            dir: &dir,
            stated: stated.unwrap_or(&now_stated),
        },
        signer.dir(),
        signer.key_id(),
        own,
    )
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
    let evidence = gather_for(&s, signer, None, Ok(own_store));
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
/// The live generation witness a child guards and watches, when its test gives it one.
const CHILD_GENERATION: &str = "PODMESH_VOTE_TEST_CHILD_GENERATION";

/// Not a test by itself: returns at once unless a parent test runs it as a child process.
#[test]
fn child_signs_one_vote() {
    let Ok(spec) = std::env::var(CHILD) else {
        return;
    };
    let spec: Value = serde_json::from_str(&spec).unwrap();
    let dir = PathBuf::from(spec["dir"].as_str().unwrap());
    let mut signer = signer_a(&dir);
    let now = spec["now"].as_i64().unwrap();
    // The witness, guarded before the signing lock and watched through it, as the resident's own
    // signer holds it: the parent moves it while this child is inside the lock.
    if let Ok(path) = std::env::var(CHILD_GENERATION) {
        let path = PathBuf::from(path);
        let generation = crate::votes::generation_id_from(&path).unwrap();
        signer.guard_generation(&generation, now).unwrap();
        signer.watch_generation_unchecked(path, &generation);
    }
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

/// A file-based VM restore rewinds the marker and ledger together. The next kernel boot must
/// quarantine the ledger before the resident can exchange with peers, even if no later vote has
/// survived elsewhere to trip the ordinary vote-history check.
#[test]
fn a_restored_vm_boot_is_unadmitted_before_voting() {
    const FIRST_BOOT: &str = "11111111-2222-4333-8444-555555555555";
    const NEXT_BOOT: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    let root = temp_root();
    let dir = private_dir(root.path(), "a");
    let signer = admitted_a(root.path(), &dir);
    // Snapshot an admitted host whose marker belongs to the first boot. A same-boot process
    // restart keeps its admission; restoring those exact bytes under a new boot quarantines it.
    let marker = dir.join("replica-a.boot-id");
    fs::write(&marker, FIRST_BOOT).unwrap();
    fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).unwrap();
    signer.guard_boot(FIRST_BOOT, T1).unwrap();
    assert!(signer.load().unwrap().admitted);
    signer.guard_boot(NEXT_BOOT, T1 + 2).unwrap();
    assert!(!signer.load().unwrap().admitted);
    assert_eq!(signer.load().unwrap().unadmitted_since, Some(T1 + 2));
    assert_eq!(fs::read_to_string(&marker).unwrap(), NEXT_BOOT);
    signer.guard_boot(NEXT_BOOT, T1 + 3).unwrap();
    assert_eq!(signer.load().unwrap().unadmitted_since, Some(T1 + 2));
    assert_eq!(
        code(signer.sign(&takeover(&policy(ABC, 0), R, 5, X, T1 + 3), &[], T1 + 3)),
        "ledger_unadmitted"
    );
    assert_eq!(
        code(signer.guard_boot("invalid", T1 + 3)),
        "boot_identity_unreadable"
    );

    // An older admitted ledger without a marker (upgrade or incomplete copy) fails closed too.
    let other = private_dir(root.path(), "b");
    let legacy = admitted_a(root.path(), &other);
    legacy.guard_boot(FIRST_BOOT, T1 + 4).unwrap();
    assert!(!legacy.load().unwrap().admitted);
}

#[test]
fn a_memory_snapshot_resume_changes_the_external_generation_witness() {
    const OLD: &str = "11111111222243338444555555555555";
    const NEW: &str = "aaaaaaaaaaaa4ccc8dddeeeeeeeeeeee";
    let root = temp_root();
    let dir = private_dir(root.path(), "a");
    let signer = admitted_a(root.path(), &dir);
    let marker = dir.join("replica-a.generation-id");
    fs::write(&marker, OLD).unwrap();
    fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).unwrap();
    signer.guard_generation(OLD, T1).unwrap();
    assert!(signer.load().unwrap().admitted);
    // The restored process and disk can keep the original boot ID. A fresh hypervisor value
    // still quarantines before any vote operation can use this signer.
    signer.guard_generation(NEW, T1 + 1).unwrap();
    assert_eq!(signer.load().unwrap().unadmitted_since, Some(T1 + 1));
    assert_eq!(
        code(signer.sign(&takeover(&policy(ABC, 0), R, 5, X, T1 + 2), &[], T1 + 2)),
        "ledger_unadmitted"
    );
    assert_eq!(fs::read_to_string(marker).unwrap(), NEW);
}

/// A live hypervisor generation witness: the 16 bytes `byte`, as a platform adapter supplies the
/// bare item. Returns the identifier a signer reads from it.
fn write_generation(path: &Path, byte: u8) -> String {
    fs::write(path, [byte; 16]).unwrap();
    quorum::hex(&[byte; 16])
}

/// The marker `guard_generation` compares the witness to, as a host that never moved generation
/// would have left it: so the guard passes and quarantines nothing.
fn place_generation_marker(dir: &Path, key_id: &str, generation: &str) {
    let marker = dir.join(format!("{key_id}.generation-id"));
    fs::write(&marker, generation).unwrap();
    fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).unwrap();
}

/// Proves (the guard's time-of-check gap): the generation is guarded before the signing lock is
/// taken and the lock released again, so a signer that watches the witness reads it once more under
/// that lock, in the last moment before its vote is sealed. A witness that moved between the guard
/// and the signature releases nothing, names `generation_changed` and marks the ledger unadmitted,
/// durably; before this, a signature was released on the strength of a check the snapshot resume had
/// undone. A witness that cannot be read shows nothing about the generation either: it releases
/// nothing, and quarantines nothing by itself.
#[test]
fn a_generation_that_moves_after_the_guard_releases_no_signature() {
    let root = temp_root();
    let dir = private_dir(root.path(), "a");
    let mut signer = admitted_a(root.path(), &dir);
    let q = policy(ABC, 0);
    let witness = root.path().join("generation");
    let guarded = write_generation(&witness, 1);
    place_generation_marker(&dir, "replica-a", &guarded);
    signer.guard_generation(&guarded, T1).unwrap();
    signer.watch_generation_unchecked(witness.clone(), &guarded);
    // On the generation guarded, the vote is signed as before.
    signer.sign(&takeover(&q, R, 5, X, T1), &[], T1).unwrap();

    // The hypervisor resumes the guest from a snapshot: this process's memory, the ledger and the
    // marker go back together, and only the witness it holds outside the snapshot says so.
    write_generation(&witness, 2);
    assert_eq!(
        code(signer.sign(&takeover(&q, R, 6, X, T1), &[], T1)),
        ledger::GENERATION_CHANGED
    );
    let quarantined = signer.load().unwrap();
    assert!(!quarantined.admitted);
    assert_eq!(quarantined.unadmitted_since, Some(T1));
    assert!(quarantined
        .unadmitted_reason
        .as_deref()
        .unwrap()
        .starts_with("generation witness (generation_changed)"));
    assert_eq!(
        code(signer_a(&dir).sign(&takeover(&q, R, 6, X, T1 + 1), &[], T1 + 1)),
        "ledger_unadmitted",
        "durably: another signer on these files signs nothing either"
    );

    let second = private_dir(root.path(), "b");
    let mut other = admitted_a(root.path(), &second);
    let guarded = write_generation(&witness, 3);
    place_generation_marker(&second, "replica-a", &guarded);
    other.guard_generation(&guarded, T1).unwrap();
    other.watch_generation_unchecked(witness.clone(), &guarded);
    fs::remove_file(&witness).unwrap();
    assert_eq!(
        code(other.sign(&takeover(&q, R, 5, X, T1), &[], T1)),
        "generation_identity_unreadable"
    );
    assert!(
        other.load().unwrap().admitted,
        "an unreadable witness quarantines nothing by itself"
    );
}

/// Proves: the witness is read inside the signing lock, not before it. A child signer is paused
/// after its checks, holding the lock, while the generation moves under it; it releases no
/// signature, answers `generation_changed`, and leaves the ledger unadmitted on disk.
#[test]
fn a_generation_that_moves_inside_the_signing_lock_releases_no_signature() {
    let root = temp_root();
    let dir = private_dir(root.path(), "a");
    let signer = admitted_a(root.path(), &dir);
    let q = policy(ABC, 0);
    let witness = root.path().join("generation");
    let guarded = write_generation(&witness, 1);
    place_generation_marker(&dir, "replica-a", &guarded);
    let mark = root.path().join("child-paused");
    let mut child = run_child(
        &dir,
        "generation",
        &takeover(&q, R, 9, X, T1),
        T1,
        &[
            ("PODMESH_VOTE_TEST_PAUSE", "after_check".to_string()),
            ("PODMESH_VOTE_TEST_PAUSE_MS", "1500".to_string()),
            ("PODMESH_VOTE_TEST_PAUSE_MARK", mark.display().to_string()),
            (CHILD_GENERATION, witness.display().to_string()),
        ],
    );
    let started = std::time::Instant::now();
    while !mark.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the child never reached its pause"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    write_generation(&witness, 2);
    assert!(child.wait().unwrap().success());
    assert_eq!(
        fs::read_to_string(dir.join("outcome-generation")).unwrap(),
        ledger::GENERATION_CHANGED
    );
    assert!(
        released(&dir, "generation").is_none(),
        "a signature was released after the generation moved under the lock"
    );
    assert!(!signer.load().unwrap().admitted);
}

/// Proves (the same gap on the readmission path): the witness is read once more before the ledger
/// becomes admitted, so a generation that moved while the operator's evidence was being read admits
/// nothing. Back on the generation guarded, the same readmission admits.
#[test]
fn a_generation_that_moves_before_an_admission_admits_nothing() {
    let root = temp_root();
    let dir = private_dir(root.path(), "a");
    place_key(&dir, "replica-a", 1);
    let mut signer = signer_a(&dir);
    signer.init(T0).unwrap();
    empty_world(root.path(), &dir, T0);
    let witness = root.path().join("generation");
    let guarded = write_generation(&witness, 1);
    place_generation_marker(&dir, "replica-a", &guarded);
    signer.guard_generation(&guarded, T1).unwrap();
    signer.watch_generation_unchecked(witness.clone(), &guarded);

    write_generation(&witness, 2);
    assert_eq!(
        readmit_with(&signer, vec![], &[], T1).unwrap_err().code,
        ledger::GENERATION_CHANGED
    );
    assert!(!signer.load().unwrap().admitted);
    write_generation(&witness, 1);
    readmit_with(&signer, vec![], &[], T1).unwrap();
    assert!(signer.load().unwrap().admitted);
}

#[test]
fn the_hypervisor_generation_reader_accepts_only_bounded_nonzero_items() {
    let root = temp_root();
    let path = root.path().join("generation");
    let id: Vec<u8> = (1..=16).collect();
    fs::write(&path, &id).unwrap();
    let expected = quorum::hex(&id);
    assert_eq!(crate::votes::generation_id_from(&path).unwrap(), expected);
    let mut fw_cfg = vec![0; 4096];
    fw_cfg[40..56].copy_from_slice(&id);
    fs::write(&path, fw_cfg).unwrap();
    assert_eq!(crate::votes::generation_id_from(&path).unwrap(), expected);
    fs::write(&path, vec![0; 4096]).unwrap();
    assert_eq!(
        code(crate::votes::generation_id_from(&path)),
        "generation_identity_unreadable"
    );
    fs::write(&path, vec![7; 4097]).unwrap();
    assert_eq!(
        code(crate::votes::generation_id_from(&path)),
        "generation_identity_unreadable"
    );
}

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
/// 9 at once, one for X and one for Y, and each pauses after reading the ledger and before the
/// promise check (300 ms and 900 ms), long enough for both to read it before either writes if
/// nothing kept them apart. Exactly one signs; the other is refused `epoch_already_promised`.
/// Repeated three times.
#[test]
fn two_signers_on_one_ledger_are_serialized_by_its_lock() {
    for round in 0..3 {
        let root = temp_root();
        let dir = private_dir(root.path(), "a");
        let _signer = admitted_a(root.path(), &dir);
        let q = policy(ABC, 0);
        // Staggered: unlocked, x would write and sign while y, which read the ledger before x wrote,
        // still waits; y would then write its own stale view over x's promise and sign too.
        let pause = |ms: u32| {
            [
                ("PODMESH_VOTE_TEST_PAUSE", "after_check".to_string()),
                ("PODMESH_VOTE_TEST_PAUSE_MS", ms.to_string()),
            ]
        };
        let mut x = run_child(&dir, "x", &takeover(&q, R, 9, X, T1), T1, &pause(300));
        let mut y = run_child(&dir, "y", &takeover(&q, R, 9, Y, T1), T1, &pause(900));
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
    let later = T1 + LIFE + 61;
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
    let evidence = gather_for(&s, signer, None, Ok(own_store));
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
    let evidence_dir = evidence_dir_of(&dir);
    // The longest life plus the bound on the skew between two clocks, 60 s.
    let ready = marked + LIFE + 60;
    let refused = |expected_input: &str, expected_source: &str, own: Result<Vec<Fact>, String>| {
        let m = manager();
        let keys = replica_keys();
        let nodes = vec![NODE.to_string()];
        let s = scope(&m, &keys, &nodes, signer.policy(), &[]);
        let evidence = gather_for(&s, &signer, None, own);
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

// ---------------------------------------------------------------------------------------------
// The counter-review of V3-4: evidence the replica cannot substitute, a read-only host identity,
// a lock that cannot be replaced under a live signer, and the wait's skew bound.

/// A read-only bind mount of `paths`, held by a helper process in its own user and mount
/// namespace; this process reaches it through `/proc/<helper>/root`. `None` where unprivileged user
/// namespaces are not available, and the caller says it skipped.
pub(crate) struct ReadOnlyMounts {
    helper: std::process::Child,
}

impl ReadOnlyMounts {
    pub(crate) fn new(paths: &[&Path]) -> Option<Self> {
        let mut script = String::new();
        for p in paths {
            let p = p.display();
            script.push_str(&format!(
                "mount --bind '{p}' '{p}' && mount -o remount,bind,ro '{p}' && "
            ));
        }
        script.push_str("echo ready && exec sleep 600");
        let mut helper = Command::new("unshare")
            .args(["-rm", "sh", "-c", &script])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        let mut line = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(helper.stdout.take()?),
            &mut line,
        )
        .ok()?;
        if line.trim() != "ready" {
            let _ = helper.kill();
            let _ = helper.wait();
            return None;
        }
        Some(Self { helper })
    }

    /// `path` as this process sees it on the read-only mount.
    pub(crate) fn view(&self, path: &Path) -> PathBuf {
        PathBuf::from(format!("/proc/{}/root{}", self.helper.id(), path.display()))
    }
}

impl Drop for ReadOnlyMounts {
    fn drop(&mut self) {
        let _ = self.helper.kill();
        let _ = self.helper.wait();
    }
}

/// Proves (finding 1): readmission reads only evidence whose SHA-256 the operator stated. A peer's
/// store replaced after the digests were stated by a shorter history that is consistent in itself
/// (its own digest recomputed, the co-signed epoch dropped) is refused
/// `readmission_evidence_mismatch`, naming it; so is a digest stated for a file readmission does not
/// read; a required file with no digest stated is unreadable. The ledger stays unadmitted. With the
/// digests of what is there, the same shorter history would have been admitted with a floor below
/// the epoch it dropped: the digest is what catches it.
#[test]
fn evidence_substituted_after_its_digest_was_stated_is_refused() {
    let root = temp_root();
    let dir = private_dir(root.path(), "a");
    place_key(&dir, "replica-a", 1);
    let signer = signer_a(&dir);
    signer.init(T0).unwrap();
    let q = policy(ABC, 0);
    let dir_b = private_dir(root.path(), "b-live");
    place_key(&dir_b, "replica-b", 2);
    let b = open(&dir_b, "replica-b", HOST_OTHER, policy(ABC, 0));
    b.init(T0).unwrap();
    admit_by_hand(&b);
    let b4 = b.sign(&takeover(&q, R, 4, X, T0), &[], T0).unwrap();
    let c_ledger = peer_ledger(root.path(), "replica-c", 3);
    // The operator collected replica-b's store, which holds its vote for epoch 4, and an empty
    // ledger of replica-b's key from elsewhere: only the store shows epoch 4.
    write_evidence(
        &dir,
        T0,
        &[("r-b", vote_facts("r-b", &[b4])), ("r-c", vec![])],
        &[
            ("replica-b", peer_ledger(root.path(), "replica-b", 2)),
            ("replica-c", c_ledger),
        ],
        &[],
    );
    let evidence = evidence_dir_of(&dir);
    let stated = stated_digests(&evidence);
    // The substitute: replica-b's store with its vote for epoch 4 dropped, consistent in itself.
    write_evidence_store(&evidence, "r-b", T0, &[]);
    let m = manager();
    let keys = replica_keys();
    let nodes = vec![NODE.to_string()];
    let s = scope(&m, &keys, &nodes, signer.policy(), &[]);
    let e = readmission::readmit(
        &signer,
        &s,
        gather_for(&s, &signer, Some(&stated), Ok(vec![])),
        "r",
        Some(0),
        T1,
    )
    .unwrap_err();
    assert_eq!(e.code, "readmission_evidence_mismatch", "{e}");
    assert!(
        e.unreadable
            .iter()
            .any(|u| u.input == "store" && u.source == "r-b"),
        "{e}"
    );
    assert!(!signer.load().unwrap().admitted);
    // A digest stated for a file readmission does not read.
    let mut extra = stated_digests(&evidence);
    extra.insert("store.r-z.json".into(), "00".repeat(32));
    let e = readmission::readmit(
        &signer,
        &s,
        gather_for(&s, &signer, Some(&extra), Ok(vec![])),
        "r",
        Some(0),
        T1,
    )
    .unwrap_err();
    assert_eq!(e.code, "readmission_evidence_mismatch", "{e}");
    // A required file with no digest stated.
    let mut short = stated_digests(&evidence);
    short.remove("screen.11111111-2222-4333-8444-555555555555.json");
    let e = readmission::readmit(
        &signer,
        &s,
        gather_for(&s, &signer, Some(&short), Ok(vec![])),
        "r",
        Some(0),
        T1,
    )
    .unwrap_err();
    assert_eq!(e.code, "readmission_inputs_unreadable", "{e}");
    assert!(
        e.unreadable
            .iter()
            .any(|u| u.input == "screen" && u.reason.contains("no SHA-256 was stated")),
        "{e}"
    );
    // What the digest caught: vouched for as it is now, the substitute admits below epoch 4.
    let admitted = readmission::readmit(
        &signer,
        &s,
        gather_for(&s, &signer, None, Ok(vec![])),
        "r",
        Some(0),
        T1,
    )
    .unwrap();
    assert!(
        admitted.floors_set.get(R).is_none_or(|f| f.epoch_floor < 4),
        "{admitted:?}"
    );
}

fn write_evidence_store(evidence: &Path, replica: &str, at: i64, facts: &[Fact]) {
    let m = manager();
    let content = json!({"history_count": facts.len(), "ordered_facts": facts,
                         "logical_history_sha256": facts_history_sha256(&m, facts).unwrap()});
    write_one(evidence, "store", replica, at, content);
}

/// Proves (finding 1): evidence placed in the vote directory, which the replica writes, is never
/// read. Complete, correct evidence under `<vote_dir>/readmission/`, with its digests stated, and an
/// empty evidence directory: readmission names every input missing from the evidence directory, and
/// reads nothing from the vote directory.
#[test]
fn evidence_placed_in_the_vote_directory_is_ignored() {
    let root = temp_root();
    let dir = private_dir(root.path(), "a");
    place_key(&dir, "replica-a", 1);
    let signer = signer_a(&dir);
    signer.init(T0).unwrap();
    empty_world(root.path(), &dir, T0);
    let evidence = evidence_dir_of(&dir);
    let inside = dir.join("readmission");
    fs::rename(&evidence, &inside).unwrap();
    private(&evidence);
    let stated = stated_digests(&inside);
    assert_eq!(stated.len(), 5);
    let m = manager();
    let keys = replica_keys();
    let nodes = vec![NODE.to_string()];
    let s = scope(&m, &keys, &nodes, signer.policy(), &[]);
    let gathered = gather_for(&s, &signer, Some(&stated), Ok(vec![]));
    assert_eq!(
        gathered.items.iter().map(|i| i.input).collect::<Vec<_>>(),
        ["own_store"]
    );
    let e = readmission::readmit(&signer, &s, gathered, "r", Some(0), T1).unwrap_err();
    assert_eq!(e.code, "readmission_inputs_unreadable", "{e}");
    assert_eq!(e.unreadable.len(), 5, "{e}");
    // And the evidence directory itself may not be the vote directory or inside it.
    let refused = readmission::check_evidence_dir(&inside, &dir, &[]).unwrap_err();
    assert_eq!(refused.code, "evidence_dir_inside_vote_dir");
    let refused = readmission::check_evidence_dir(&dir, &dir, &[]).unwrap_err();
    assert_eq!(refused.code, "evidence_dir_inside_vote_dir");
}

/// Proves (finding 1): the evidence directory must be one the replica cannot write, now or after a
/// `chmod` of its own. A directory of this user is refused even at mode 0555 with 0444 files
/// (`evidence_dir_writable`); the same directory mounted read-only is accepted; a symlinked evidence
/// file is refused.
#[test]
fn the_evidence_directory_must_be_one_the_replica_cannot_write() {
    let root = temp_root();
    let dir = private_dir(root.path(), "a");
    let evidence = private_dir(root.path(), "a-evidence");
    fs::write(evidence.join("store.r-b.json"), "{}").unwrap();
    let names = vec!["store.r-b.json".to_string()];
    let e = readmission::check_evidence_dir(&evidence, &dir, &names).unwrap_err();
    assert_eq!(e.code, "evidence_dir_writable");
    fs::set_permissions(
        evidence.join("store.r-b.json"),
        fs::Permissions::from_mode(0o444),
    )
    .unwrap();
    fs::set_permissions(&evidence, fs::Permissions::from_mode(0o555)).unwrap();
    let e = readmission::check_evidence_dir(&evidence, &dir, &names).unwrap_err();
    assert_eq!(
        e.code, "evidence_dir_writable",
        "not writable now, but this user's to chmod"
    );
    fs::set_permissions(&evidence, fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink(
        evidence.join("store.r-b.json"),
        evidence.join("screen.n.json"),
    )
    .unwrap();
    fs::set_permissions(&evidence, fs::Permissions::from_mode(0o555)).unwrap();
    let Some(mounts) = ReadOnlyMounts::new(&[&evidence]) else {
        eprintln!("SKIPPED: the read-only case needs unprivileged user namespaces (unshare -rm)");
        return;
    };
    let seen = mounts.view(&evidence);
    readmission::check_evidence_dir(&seen, &dir, &names).unwrap();
    let e =
        readmission::check_evidence_dir(&seen, &dir, &["screen.n.json".to_string()]).unwrap_err();
    assert_eq!(e.code, "evidence_dir_writable", "a symlink is refused: {e}");
    drop(mounts);
    fs::set_permissions(&evidence, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Proves (finding 4): the host's machine-id is read only from a regular file on a read-only mount.
/// The same file on a writable mount is `host_identity_not_read_only`, a symlink to it and a relative
/// path are `host_identity_unreadable`; mounted read-only it reads. A whole-VM clone carries the
/// same machine-id, read-only or not: this does not replace the rule of no VM clone (A3).
#[test]
fn the_host_identity_is_read_only_from_a_read_only_mount() {
    let root = temp_root();
    let file = root.path().join("machine-id");
    fs::write(&file, format!("{HOST_A}\n")).unwrap();
    assert_eq!(
        HostIdentity::read(&file).unwrap_err().code,
        "host_identity_not_read_only"
    );
    let link = root.path().join("machine-id-link");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    assert_eq!(
        HostIdentity::read(&link).unwrap_err().code,
        "host_identity_unreadable"
    );
    assert_eq!(
        HostIdentity::read(Path::new("machine-id"))
            .unwrap_err()
            .code,
        "host_identity_unreadable"
    );
    let Some(mounts) = ReadOnlyMounts::new(&[&file]) else {
        eprintln!("SKIPPED: the read-only case needs unprivileged user namespaces (unshare -rm)");
        return;
    };
    assert_eq!(
        HostIdentity::read(&mounts.view(&file))
            .unwrap()
            .machine_id(),
        HOST_A
    );
}

/// Proves (finding 3): a lock file removed and replaced under a live signer lets no second signer
/// in. Signer X is paused after reading the ledger, holding the lock; the lock file of the previous
/// design (`replica-a.ledger.lock`) is removed and replaced by a new file; signer Y is started.
/// Exactly one signs epoch 9, three rounds: the lock is on the vote directory, which the replacement
/// does not touch.
#[test]
fn a_replaced_lock_file_lets_no_second_signer_in() {
    for round in 0..3 {
        let root = temp_root();
        let dir = private_dir(root.path(), "a");
        let _signer = admitted_a(root.path(), &dir);
        let q = policy(ABC, 0);
        let lock_file = dir.join("replica-a.ledger.lock");
        fs::write(&lock_file, "").unwrap();
        let mark = root.path().join("x-paused");
        let mut x = run_child(
            &dir,
            "x",
            &takeover(&q, R, 9, X, T1),
            T1,
            &[
                ("PODMESH_VOTE_TEST_PAUSE", "after_check".to_string()),
                ("PODMESH_VOTE_TEST_PAUSE_MS", "1500".to_string()),
                ("PODMESH_VOTE_TEST_PAUSE_MARK", mark.display().to_string()),
            ],
        );
        let started = std::time::Instant::now();
        while !mark.exists() {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "round {round}: X never reached its pause"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = fs::remove_file(&lock_file);
        fs::write(&lock_file, "replaced").unwrap();
        let mut y = run_child(&dir, "y", &takeover(&q, R, 9, Y, T1), T1, &[]);
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
        assert_eq!(
            ["x", "y"].iter().filter_map(|t| released(&dir, t)).count(),
            1,
            "round {round}"
        );
    }
}

/// Proves (the unanswered question): a signer that has said nothing about a hypervisor generation
/// witness signs nothing and readmits nothing. Before this, the absence of a witness was silently
/// the same as a witness that agreed, so one replica started without its mount voted with no
/// defence against a snapshot rollback and nothing anywhere said so. Silence is not a waiver; the
/// operator's decision to do without is recorded on the signer and can then be read back.
#[test]
fn a_signer_that_says_nothing_about_a_witness_signs_nothing() {
    let root = temp_root();
    let dir = private_dir(root.path(), "a");
    // `admitted_a` leaves an admitted ledger on these files, and waives on its own signer.
    let _prepared = admitted_a(root.path(), &dir);
    let q = policy(ABC, 0);

    // A second signer on the same files that was never told anything about a witness.
    let mut silent = Signer::open(
        &dir,
        "replica-a",
        HostIdentity::from_machine_id(HOST_A).unwrap(),
        policy(ABC, 0),
        rules(),
    )
    .unwrap();
    assert_eq!(
        code(silent.sign(&takeover(&q, R, 5, X, T1), &[], T1)),
        ledger::GENERATION_WITNESS_ABSENT
    );
    assert!(
        silent.load().unwrap().admitted,
        "an unanswered question quarantines nothing: it is a configuration fault, not evidence that the guest went back"
    );
    assert!(
        silent.generation_waiver().is_none(),
        "nothing was waived, so nothing is reported as waived"
    );

    // The operator's recorded decision to do without, and the signature it then allows.
    silent
        .waive_generation("no hypervisor snapshots on this host: recorded by the operator")
        .unwrap();
    assert_eq!(
        silent.generation_waiver(),
        Some("no hypervisor snapshots on this host: recorded by the operator")
    );
    silent.sign(&takeover(&q, R, 5, X, T1), &[], T1).unwrap();
}

/// Proves (the witness must be one the hypervisor answers for): a witness on an ordinary filesystem
/// is refused. A rollback that restores the guest's memory restores its page cache with it, so such
/// a file answers with the value it held before the rollback and reading it again proves nothing.
/// Only a filesystem the hypervisor itself answers for -- sysfs, where the platform exposes QEMU's
/// `fw_cfg` items, including through a bind mount of one into a universe -- is accepted.
#[test]
fn a_witness_the_hypervisor_does_not_answer_for_is_refused() {
    let root = temp_root();
    let dir = private_dir(root.path(), "a");
    let mut signer = admitted_a(root.path(), &dir);
    let witness = root.path().join("generation");
    let guarded = write_generation(&witness, 1);

    let refused = signer
        .watch_generation(witness.clone(), &guarded)
        .unwrap_err();
    assert_eq!(refused.code, ledger::GENERATION_WITNESS_UNTRUSTED);
    assert!(
        refused.detail.contains("page cache"),
        "the refusal says why an ordinary file cannot be the witness: {}",
        refused.detail
    );

    let absent = signer
        .watch_generation(root.path().join("no-such-witness"), &guarded)
        .unwrap_err();
    assert_eq!(absent.code, ledger::GENERATION_WITNESS_UNTRUSTED);

    // A file the kernel answers for is accepted. Any sysfs path proves the filesystem test; the
    // deployed witness is QEMU's `fw_cfg` item, which lives on the same filesystem.
    let sysfs = std::path::Path::new("/sys/devices/system/cpu/online");
    if sysfs.exists() {
        signer
            .watch_generation(sysfs.to_path_buf(), &guarded)
            .expect("sysfs is a filesystem the hypervisor answers for");
    }
}

/// Proves (nothing promised for a vote that was refused outright): when the witness has already
/// moved by the time a signature is asked for, the refusal comes before the ledger is written, so
/// no promise is left pinning that resource's number. Before this, the witness was read only after
/// the promise had been stored, and a refusal left the ledger holding a number against every other
/// holder while no vote had been released for it.
#[test]
fn a_witness_that_already_moved_leaves_no_promise_behind() {
    let root = temp_root();
    let dir = private_dir(root.path(), "a");
    let mut signer = admitted_a(root.path(), &dir);
    let q = policy(ABC, 0);
    let witness = root.path().join("generation");
    let guarded = write_generation(&witness, 1);
    place_generation_marker(&dir, "replica-a", &guarded);
    signer.guard_generation(&guarded, T1).unwrap();
    signer.watch_generation_unchecked(witness.clone(), &guarded);
    signer.sign(&takeover(&q, R, 5, X, T1), &[], T1).unwrap();
    let settled = signer.load().unwrap();

    // The guest went back before the next signature was even asked for.
    write_generation(&witness, 2);
    assert_eq!(
        code(signer.sign(&takeover(&q, R, 6, X, T1), &[], T1)),
        ledger::GENERATION_CHANGED
    );

    let after = signer.load().unwrap();
    assert!(!after.admitted, "the ledger is quarantined, durably");
    assert_eq!(
        after.sequence, settled.sequence,
        "the refused signature issued no sequence number"
    );
    assert_eq!(
        after.resources, settled.resources,
        "and promised nothing: epoch 6 is still free for whoever the operator readmits this replica to serve"
    );
}

/// Proves (the witness's own forms and their refusals): what a witness may be, and what it may not.
/// QEMU's `fw_cfg` item is 4096 bytes with the generation at offset 40; a platform adapter may
/// supply the bare 16. Everything else refuses rather than being interpreted: a length that is
/// neither, an all-zero item that means the hypervisor gave nothing, and a file that is not there.
/// A sysfs path is a filesystem the hypervisor answers for, which does not make every sysfs file a
/// generation item: `/sys/devices/system/cpu/online` passes the filesystem test and is still
/// unreadable as a witness. Nothing here writes a marker or touches a ledger.
#[test]
fn a_witness_is_one_of_two_forms_and_nothing_else() {
    let root = temp_root();

    let bare = root.path().join("bare");
    let expected = write_generation(&bare, 7);
    assert_eq!(crate::votes::generation_id_from(&bare).unwrap(), expected);

    // The fw_cfg item: 4096 bytes, the generation at offset 40, the rest none of our business.
    let fw_cfg = root.path().join("fw-cfg");
    let mut item = vec![0xAA_u8; 4096];
    item[40..56].copy_from_slice(&[9_u8; 16]);
    fs::write(&fw_cfg, &item).unwrap();
    assert_eq!(
        crate::votes::generation_id_from(&fw_cfg).unwrap(),
        quorum::hex(&[9_u8; 16]),
        "the 16 bytes at offset 40, not the bytes around them"
    );

    let wrong_length = root.path().join("wrong-length");
    fs::write(&wrong_length, [1_u8; 15]).unwrap();
    assert_eq!(
        code(crate::votes::generation_id_from(&wrong_length)),
        "generation_identity_unreadable",
        "a length that is neither form is refused, not padded or truncated into one"
    );

    let zero = root.path().join("zero");
    fs::write(&zero, [0_u8; 16]).unwrap();
    assert_eq!(
        code(crate::votes::generation_id_from(&zero)),
        "generation_identity_unreadable",
        "an all-zero item is the hypervisor saying nothing"
    );

    assert_eq!(
        code(crate::votes::generation_id_from(&root.path().join("absent"))),
        "generation_identity_unreadable"
    );

    // Sysfs is the filesystem, not the item. A sysfs file that is not a generation item passes the
    // filesystem test and is refused when it is read, which is where the two checks divide.
    let sysfs = std::path::Path::new("/sys/devices/system/cpu/online");
    if sysfs.exists() {
        ledger::trust_generation_witness(sysfs).expect("sysfs is a filesystem worth reading again");
        assert_eq!(
            code(crate::votes::generation_id_from(sysfs)),
            "generation_identity_unreadable",
            "and it is still not a generation item"
        );
    }
}

/// Proves (a path that could never be a witness marks nothing): whether a path can be a witness at
/// all is settled before `guard_generation`, which persists a marker and can mark the ledger
/// unadmitted. Before this, naming an unusable path still wrote the marker and could quarantine the
/// ledger durably; a typo in one environment variable was enough to stop a replica voting until an
/// operator readmitted it.
#[test]
fn a_path_that_could_never_be_a_witness_marks_nothing() {
    let root = temp_root();
    let dir = private_dir(root.path(), "a");
    let signer = admitted_a(root.path(), &dir);
    let marker = dir.join("replica-a.generation-id");

    let ordinary = root.path().join("generation");
    write_generation(&ordinary, 1);
    let refused = ledger::trust_generation_witness(&ordinary).unwrap_err();
    assert_eq!(refused.code, ledger::GENERATION_WITNESS_UNTRUSTED);

    let absent = ledger::trust_generation_witness(&root.path().join("typo")).unwrap_err();
    assert_eq!(
        absent.code,
        ledger::GENERATION_WITNESS_UNTRUSTED,
        "a path that is not there cannot be read again either"
    );

    assert!(
        !marker.exists(),
        "no marker was written for a path that was refused before it was read"
    );
    assert!(
        signer.load().unwrap().admitted,
        "and the ledger was not quarantined by the attempt"
    );
}

/// A replica of the quorum with its own directory, key, admitted ledger and its own live generation
/// witness, as three guests of one hypervisor would each have. Returns the signer and its witness.
fn watching_replica(root: &Path, key_id: &str, seed: u8) -> (Signer, std::path::PathBuf) {
    let dir = private_dir(root, key_id);
    place_key(&dir, key_id, seed);
    let mut signer = Signer::open(
        &dir,
        key_id,
        HostIdentity::from_machine_id(HOST_A).unwrap(),
        policy(ABC, 0),
        rules(),
    )
    .unwrap();
    signer.init(T0).unwrap();
    admit_by_hand(&signer);
    let witness = root.join(format!("{key_id}.generation"));
    let guarded = write_generation(&witness, seed);
    place_generation_marker(&dir, key_id, &guarded);
    signer.guard_generation(&guarded, T1).unwrap();
    signer.watch_generation_unchecked(witness.clone(), &guarded);
    (signer, witness)
}

/// Proves (MED-M5 gate 2, at the quorum): a witness that moves between candidate signatures costs
/// the quorum exactly that voter, and nothing else. The two whose guests did not move still decide,
/// because 2 of 3 is the threshold and the third is not needed; the one that moved releases nothing
/// for the new epoch, is durably unadmitted and stays so across a restart; and its earlier vote, for
/// the earlier epoch, cannot be made to stand for the new one. A quorum that loses a second voter
/// this way decides nothing, which is the direction a rollback must push a cluster.
///
/// This is three signers on one workstation, not three guests: it proves the rule, not the field.
#[test]
fn a_witness_that_moves_between_candidate_signatures_costs_the_quorum_that_voter() {
    let root = temp_root();
    let q = policy(ABC, 0);
    let (a, witness_a) = watching_replica(root.path(), "replica-a", 1);
    let (b, _witness_b) = watching_replica(root.path(), "replica-b", 2);
    let (c, witness_c) = watching_replica(root.path(), "replica-c", 3);

    // Epoch 12: every guest is on the generation it was guarded on, and any two of the three decide.
    let first = takeover(&q, R, 12, X, T1);
    let (va, vb) = (
        a.sign(&first, &[], T1).unwrap(),
        b.sign(&first, &[], T1).unwrap(),
    );
    let certificate = vote::assemble(&q, &first, &[va.clone(), vb.clone()]).unwrap();
    assert_eq!(
        q.verify(&certificate, QUORUM_PROOF_KIND, TAKEOVER_FIELDS)
            .unwrap(),
        ["replica-a", "replica-b"]
    );

    // Between the candidate signatures, a's guest is resumed from a snapshot.
    write_generation(&witness_a, 9);

    let second = takeover(&q, R, 13, X, T1);
    assert_eq!(
        code(a.sign(&second, &[], T1)),
        ledger::GENERATION_CHANGED,
        "the voter whose guest moved releases nothing"
    );
    assert!(!a.load().unwrap().admitted, "and is quarantined, durably");

    // The other two decide the new epoch without it: the quorum is two, not three.
    let (vb2, vc2) = (
        b.sign(&second, &[], T1).unwrap(),
        c.sign(&second, &[], T1).unwrap(),
    );
    let second_certificate = vote::assemble(&q, &second, &[vb2, vc2]).unwrap();
    assert_eq!(
        q.verify(&second_certificate, QUORUM_PROOF_KIND, TAKEOVER_FIELDS)
            .unwrap(),
        ["replica-b", "replica-c"]
    );

    // a's earlier vote belongs to epoch 12 and cannot be counted for 13: assembly reads the payload
    // each signature was made over, not the number of signatures collected.
    assert!(
        vote::assemble(&q, &second, &[va, vb]).is_err(),
        "a vote for another payload is not a vote for this one"
    );

    // Nor does a come back by restarting: another signer on those files signs nothing either.
    let mut restarted = Signer::open(
        &private_dir(root.path(), "replica-a"),
        "replica-a",
        HostIdentity::from_machine_id(HOST_A).unwrap(),
        policy(ABC, 0),
        rules(),
    )
    .unwrap();
    restarted
        .waive_generation("restart of the quarantined replica, for this check only")
        .unwrap();
    assert_eq!(
        code(restarted.sign(&takeover(&q, R, 14, X, T1 + 1), &[], T1 + 1)),
        "ledger_unadmitted"
    );

    // A second guest resumed leaves one voter: below the threshold, nothing is decided.
    write_generation(&witness_c, 9);
    let third = takeover(&q, R, 15, X, T1);
    assert_eq!(code(c.sign(&third, &[], T1)), ledger::GENERATION_CHANGED);
    let vb3 = b.sign(&third, &[], T1).unwrap();
    assert!(
        vote::assemble(&q, &third, &[vb3]).is_err(),
        "one voter of three decides nothing"
    );
}

/// Proves (MED-M5 gate 2, the deadline): what the witness costs a signature is far below the vote
/// control deadline the resident answers within. The two reads the signing path now makes are two
/// opens of a small file under a lock already held; the measurement here is the whole `sign`, key
/// and all. It is a floor, not a promise: a laboratory guest reading a `fw_cfg` item through a bind
/// mount is slower than a workstation reading a local file, and the field measurement belongs to
/// the campaign record, not to this suite.
#[test]
fn a_signature_costs_far_less_than_the_vote_deadline() {
    let root = temp_root();
    let q = policy(ABC, 0);
    let (signer, _witness) = watching_replica(root.path(), "replica-a", 1);

    let mut worst = std::time::Duration::ZERO;
    for epoch in 12..32 {
        let payload = takeover(&q, R, epoch, X, T1);
        let started = std::time::Instant::now();
        signer.sign(&payload, &[], T1).unwrap();
        worst = worst.max(started.elapsed());
    }

    let deadline = crate::VOTE_CONTROL_DEADLINE;
    assert!(
        worst * 20 < deadline,
        "the slowest of twenty signatures was {worst:?}, which is not comfortably inside the {deadline:?} vote control deadline"
    );
    println!("gate 2: slowest of twenty signatures {worst:?}, vote control deadline {deadline:?}");
}

/// Proves (the unanswered question reaches readmission too): a signer that has said nothing about a
/// witness readmits nothing, not only signs nothing. The earlier suite proved the signing half and
/// the docstring claimed both; a counter-review noticed that the readmission half was never
/// exercised. It is the half that matters after a restore, when readmission is the only way back.
#[test]
fn a_signer_that_says_nothing_about_a_witness_readmits_nothing() {
    let root = temp_root();
    let dir = private_dir(root.path(), "a");
    place_key(&dir, "replica-a", 1);
    let silent = Signer::open(
        &dir,
        "replica-a",
        HostIdentity::from_machine_id(HOST_A).unwrap(),
        policy(ABC, 0),
        rules(),
    )
    .unwrap();
    silent.init(T0).unwrap();
    empty_world(root.path(), &dir, T0);

    let refused = readmit_with(&silent, vec![], &[], T1).unwrap_err();
    assert_eq!(refused.code, ledger::GENERATION_WITNESS_ABSENT);
    assert!(
        !silent.load().unwrap().admitted,
        "and the ledger stays unadmitted, which is the safe direction"
    );

    // The same signer, once the operator has recorded why it does without, readmits.
    let mut spoken = Signer::open(
        &dir,
        "replica-a",
        HostIdentity::from_machine_id(HOST_A).unwrap(),
        policy(ABC, 0),
        rules(),
    )
    .unwrap();
    spoken
        .waive_generation("bare-metal host, no hypervisor can snapshot it: recorded by the operator")
        .unwrap();
    readmit_with(&spoken, vec![], &[], T1).unwrap();
    assert!(spoken.load().unwrap().admitted);
}

/// Proves (a waiver with no reason is not a waiver): the empty string is refused at the signer, not
/// only at the tool that writes configurations. A counter-review pointed out that the tool's check
/// was the only one, so a crate caller could waive with nothing at all.
#[test]
fn a_waiver_with_no_reason_is_refused() {
    let root = temp_root();
    let dir = private_dir(root.path(), "a");
    place_key(&dir, "replica-a", 1);
    let mut signer = Signer::open(
        &dir,
        "replica-a",
        HostIdentity::from_machine_id(HOST_A).unwrap(),
        policy(ABC, 0),
        rules(),
    )
    .unwrap();
    for empty in ["", "   ", "\n\t "] {
        assert_eq!(
            signer.waive_generation(empty).unwrap_err().code,
            ledger::GENERATION_WITNESS_ABSENT
        );
    }
    assert!(
        signer.generation_waiver().is_none(),
        "a refused waiver leaves the signer as it was: still saying nothing"
    );
    signer.waive_generation("a reason").unwrap();
    assert_eq!(signer.generation_waiver(), Some("a reason"));
}

/// Proves (the witness is judged again, not once): a path is not a file. A witness swapped since it
/// was accepted — a file replaced, a symlink flipped, a mount laid over it — is refused, because the
/// check that accepted it was made once at startup and the guest has had the time since. Before
/// this, the value of whatever now sat at that path was compared as though it were the witness.
#[test]
fn a_witness_swapped_since_it_was_accepted_is_refused() {
    let root = temp_root();
    let dir = private_dir(root.path(), "a");
    let mut signer = admitted_a(root.path(), &dir);
    let q = policy(ABC, 0);
    let witness = root.path().join("generation");
    let guarded = write_generation(&witness, 1);
    place_generation_marker(&dir, "replica-a", &guarded);
    signer.guard_generation(&guarded, T1).unwrap();
    signer.watch_generation_unchecked(witness.clone(), &guarded);
    signer.sign(&takeover(&q, R, 5, X, T1), &[], T1).unwrap();

    // Another file, holding the very value that was guarded, put in its place.
    let impostor = root.path().join("impostor");
    fs::write(&impostor, fs::read(&witness).unwrap()).unwrap();
    fs::rename(&impostor, &witness).unwrap();

    assert_eq!(
        code(signer.sign(&takeover(&q, R, 6, X, T1), &[], T1)),
        ledger::GENERATION_WITNESS_UNTRUSTED,
        "the same value from another file is not the witness that was accepted"
    );
    assert!(
        signer.load().unwrap().admitted,
        "a witness that was swapped says nothing about the generation, so it quarantines nothing"
    );
}

/// Proves (the ordered adoption marks nothing on a path it refuses): the whole act — judge the path,
/// read it, guard the ledger, watch it — refuses at the first step for a path that could never be a
/// witness, and leaves no marker behind. The earlier suite called only the judging function and so
/// proved the error code rather than the order; a counter-review said so.
#[test]
fn adopting_a_path_that_could_never_be_a_witness_marks_nothing() {
    let root = temp_root();
    let dir = private_dir(root.path(), "a");
    // Deliberately not `admitted_a`, whose signer already waives: this suite is about a signer that
    // has said nothing, so it opens its own after the ledger exists.
    drop(admitted_a(root.path(), &dir));
    let mut signer = Signer::open(
        &dir,
        "replica-a",
        HostIdentity::from_machine_id(HOST_A).unwrap(),
        policy(ABC, 0),
        rules(),
    )
    .unwrap();
    let marker = dir.join("replica-a.generation-id");

    let ordinary = root.path().join("generation");
    write_generation(&ordinary, 1);
    assert_eq!(
        code(signer.adopt_generation_witness(ordinary, T1)),
        ledger::GENERATION_WITNESS_UNTRUSTED
    );
    assert_eq!(
        code(signer.adopt_generation_witness(root.path().join("typo"), T1)),
        ledger::GENERATION_WITNESS_UNTRUSTED
    );
    assert!(!marker.exists(), "no marker was written for either path");
    assert!(signer.load().unwrap().admitted, "and nothing was quarantined");

    // And the signer is still saying nothing: a refused adoption is not a waiver.
    assert_eq!(
        code(signer.sign(&takeover(&policy(ABC, 0), R, 5, X, T1), &[], T1)),
        ledger::GENERATION_WITNESS_ABSENT
    );
}
