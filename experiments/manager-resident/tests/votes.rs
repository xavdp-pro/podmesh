//! The resident's signed votes (V3-4), through its control socket, as one compiled process: the
//! ledger's operations are the operator's, a missing or unadmitted ledger signs nothing, readmission
//! refuses what it cannot read and waits, a vote is recorded in the replica's own vote scope before
//! it is answered, the promise holds across a restart of the process, the tripwire fires on a ledger
//! put back from an older copy, a ledger on another host signs nothing, and the resident's vote with
//! a second replica's assembles into a certificate the node's rules accept.
//!
//! Test-only keys from a one-byte seed; nothing here reads or writes a real key.
use ed25519_dalek::SigningKey;
use podmesh_manager_ha_lab::{
    durable::{inspect_read_only, Configuration as Manager},
    ReplicaConfig, ScopeGrant,
};
use podmesh_manager_network_lab::ConfigurationFile;
use podmesh_manager_resident_lab::{
    ledger::{HostIdentity, Signer, SigningRules},
    quorum::{Quorum, QUORUM_PROOF_KIND, TAKEOVER_FIELDS},
    readmission::EVIDENCE_FORM,
    vote,
    votes::VoteConfiguration,
    Configuration,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::{Shutdown, TcpListener},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const HOST: &str = "0123456789abcdef0123456789abcdef";
const OTHER_HOST: &str = "fedcba9876543210fedcba9876543210";
const NODE: &str = "11111111-2222-4333-8444-555555555555";
const R: &str = "91eeb6bf-5489-405b-b77a-53105b0aff7a";
const X: &str = "5d1c0b8e-3f59-4d0e-9d7a-2a1e7c4b9f10";
const Y: &str = "7e2d1c0b-4a3f-4e5d-8c7b-6a5f4e3d2c1b";
const LIFE: i64 = 5;
const KEYS: &[(&str, u8)] = &[("replica-a", 1), ("replica-b", 2), ("replica-c", 3)];

fn public(seed: u8) -> String {
    SigningKey::from_bytes(&[seed; 32])
        .verifying_key()
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn quorum_value() -> Value {
    json!({"threshold": 2, "keys": KEYS.iter().map(|(id, s)| json!({"key_id": id, "public_key": public(*s)})).collect::<Vec<_>>()})
}

fn policy() -> Quorum {
    Quorum::declared("replicas", &quorum_value()).unwrap()
}

fn now() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap()
}

fn private(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn place_key(dir: &Path, key_id: &str, seed: u8) {
    let path = dir.join(format!("{key_id}.key"));
    fs::write(&path, format!("{}\n", format!("{seed:02x}").repeat(32))).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn takeover(epoch: i64, holder: &str, at: i64) -> Value {
    let q = policy();
    json!({"kind": QUORUM_PROOF_KIND, "authority_id": "replicas", "policy_digest": q.digest(), "resource": R,
           "new_epoch": epoch, "previous_epoch": epoch - 1, "new_holder": holder, "previous_holder": null,
           "holder_boot_id": "0b7f6f1e-6a55-4c1d-8f53-1c2d3e4f5a6b", "grant_id": format!("g{epoch}"), "method": "lease_barrier",
           "eligible_after": at, "issued_at": at, "expires_at": at + LIFE})
}

struct Resident {
    dir: tempfile::TempDir,
    vote_dir: PathBuf,
    config: Configuration,
    host_file: PathBuf,
    /// The machine-id path the resident is given: the file itself, or its read-only mount.
    host_id: PathBuf,
    /// The operator's evidence directory, as the test writes it.
    evidence: PathBuf,
    mounts: Option<ReadOnlyMounts>,
    child: Option<Child>,
}

/// A read-only bind mount of `paths`, held by a helper process in its own user and mount namespace,
/// reached through `/proc/<helper>/root` (the same helper as the library tests').
struct ReadOnlyMounts {
    helper: Child,
}

impl ReadOnlyMounts {
    fn new(paths: &[&Path]) -> Option<Self> {
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
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
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

    fn view(&self, path: &Path) -> PathBuf {
        PathBuf::from(format!("/proc/{}/root{}", self.helper.id(), path.display()))
    }
}

impl Drop for ReadOnlyMounts {
    fn drop(&mut self) {
        let _ = self.helper.kill();
        let _ = self.helper.wait();
    }
}

impl Resident {
    fn new(operator_uid: u32) -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let vote_dir = dir.path().join("votes");
        private(&vote_dir);
        place_key(&vote_dir, "replica-a", 1);
        let host_file = dir.path().join("machine-id");
        fs::write(&host_file, format!("{HOST}\n")).unwrap();
        let evidence = dir.path().join("evidence");
        private(&evidence);
        let bind = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let manager = Manager {
            logical_manager_id: "votes-resident-test".into(),
            replicas: vec![ReplicaConfig {
                replica_id: "r0".into(),
                host_id: "h0".into(),
            }],
            grants: vec![
                ScopeGrant {
                    scope: "s0".into(),
                    owner_replica_id: "r0".into(),
                },
                ScopeGrant {
                    scope: vote::vote_scope("r0"),
                    owner_replica_id: "r0".into(),
                },
            ],
        };
        let uid = rustix::process::geteuid().as_raw();
        let config = Configuration {
            network: ConfigurationFile {
                replica_id: "r0".into(),
                database_path: dir.path().join("r0.sqlite"),
                manager,
                bind,
                peers: vec![],
            },
            control_socket: dir.path().join("r0.sock"),
            observation_writer_uid: uid,
            interval_ms: 100,
            max_backoff_ms: 400,
            incoming_workers: 2,
            full_verification_interval_ms: None,
            unchanged_snapshot_refresh_ms: None,
            catch_up_window_ms: None,
            votes: Some(VoteConfiguration {
                key_id: "replica-a".into(),
                authority_id: "replicas".into(),
                authority_quorum: quorum_value(),
                authority_serial: 0,
                replica_keys: BTreeMap::from([("r0".to_string(), "replica-a".to_string())]),
                retired_keys: vec![],
                nodes: vec![NODE.into()],
                evidence_dir: evidence.clone(),
                operator_uid,
                max_certificate_life_seconds: LIFE,
                require_generation_id: false,
                generation_witness_waiver: Some("test fixture on a workstation, not in a guest: no hypervisor can snapshot it".into()),
                decisions: None,
            }),
        };
        Self {
            dir,
            vote_dir,
            config,
            host_id: host_file.clone(),
            host_file,
            evidence,
            mounts: None,
            child: None,
        }
    }

    /// Gives the resident its machine-id and the evidence directory through read-only mounts, as
    /// the host would; false where user namespaces are unavailable.
    fn mount_read_only(&mut self) -> bool {
        let Some(mounts) = ReadOnlyMounts::new(&[&self.host_file, &self.evidence]) else {
            return false;
        };
        self.host_id = mounts.view(&self.host_file);
        self.config.votes.as_mut().unwrap().evidence_dir = mounts.view(&self.evidence);
        self.mounts = Some(mounts);
        true
    }

    /// The SHA-256 of every evidence file as it is now: what the operator states.
    fn digests(&self) -> Value {
        let mut stated = serde_json::Map::new();
        for entry in fs::read_dir(&self.evidence).unwrap() {
            let entry = entry.unwrap();
            let bytes = fs::read(entry.path()).unwrap();
            let digest: String = <sha2::Sha256 as sha2::Digest>::digest(&bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            stated.insert(entry.file_name().into_string().unwrap(), json!(digest));
        }
        Value::Object(stated)
    }

    fn readmit(&self, id: &str, stated: &Value) -> Value {
        self.control(&json!({"operation": "vote_ledger_readmit", "operation_id": id, "evidence_sha256": stated}))
    }

    fn start(&mut self) {
        let path = self.dir.path().join("r0.json");
        fs::write(&path, serde_json::to_vec(&self.config).unwrap()).unwrap();
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.path().join("r0.stderr"))
            .unwrap();
        self.child = Some(
            Command::new(env!("CARGO_BIN_EXE_podmesh-manager-resident-lab"))
                .env("PODMESH_MANAGER_NETWORK_MODE", "authenticated-static-peers")
                .env("PODMESH_MANAGER_VOTE_DIR", &self.vote_dir)
                .env("PODMESH_MANAGER_HOST_ID_FILE", &self.host_id)
                .arg(path)
                .stdout(Stdio::null())
                .stderr(Stdio::from(log))
                .spawn()
                .unwrap(),
        );
        let started = Instant::now();
        while self.try_control(&json!({"operation": "status"})).is_none() {
            if let Some(exit) = self.child.as_mut().unwrap().try_wait().unwrap() {
                panic!("resident exited: {exit}: {}", self.stderr());
            }
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "resident did not answer: {}",
                self.stderr()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = self.try_control(&json!({"operation": "shutdown"}));
            let deadline = Instant::now() + Duration::from_secs(10);
            while child.try_wait().unwrap().is_none() {
                if Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = fs::remove_file(&self.config.control_socket);
                    return;
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
    }

    fn stderr(&self) -> String {
        fs::read_to_string(self.dir.path().join("r0.stderr")).unwrap_or_default()
    }

    fn try_control(&self, request: &Value) -> Option<Value> {
        let mut stream = UnixStream::connect(&self.config.control_socket).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
        stream.write_all(request.to_string().as_bytes()).ok()?;
        stream.shutdown(Shutdown::Write).ok()?;
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// A control request, retried while the worker's answer was uncertain or busy: a vote operation
    /// that did not finish inside the control deadline has done what it did, and the identical
    /// request is asked again, as the README says to.
    fn control(&self, request: &Value) -> Value {
        for _ in 0..20 {
            let answer = self
                .try_control(request)
                .expect("the control socket answers");
            if !matches!(
                answer["error"].as_str(),
                Some("vote_operation_uncertain" | "vote_busy")
            ) {
                return answer;
            }
            thread::sleep(Duration::from_millis(100));
        }
        panic!("no certain answer to {request}");
    }

    fn sign(&self, id: &str, payload: &Value) -> Value {
        self.control(&json!({"operation": "vote_sign", "operation_id": id, "payload": payload}))
    }

    fn ledger_state(&self) -> Value {
        self.control(&json!({"operation": "status"}))["votes"].clone()
    }

    fn signer(&self, host: &str) -> Signer {
        Signer::open(
            &self.vote_dir,
            "replica-a",
            HostIdentity::from_machine_id(host).unwrap(),
            policy(),
            SigningRules {
                max_certificate_life: LIFE,
                lock_wait: Duration::from_secs(2),
            },
        )
        .unwrap()
    }

    /// Evidence for readmission: replica-b's and replica-c's ledgers, made by their own signers, and
    /// the node's screen, collected at `at`.
    fn write_evidence(&self, at: i64) {
        let evidence = self.evidence.clone();
        for (key, seed) in &KEYS[1..] {
            let dir = self.dir.path().join(format!("peer-{key}"));
            private(&dir);
            place_key(&dir, key, *seed);
            let signer = Signer::open(
                &dir,
                key,
                HostIdentity::from_machine_id(OTHER_HOST).unwrap(),
                policy(),
                SigningRules {
                    max_certificate_life: LIFE,
                    lock_wait: Duration::from_secs(2),
                },
            )
            .unwrap();
            let ledger = signer.load().or_else(|_| signer.init(at)).unwrap();
            write_one(
                &evidence,
                "ledger",
                key,
                at,
                serde_json::to_value(ledger).unwrap(),
            );
        }
        write_one(
            &evidence,
            "screen",
            NODE,
            at,
            json!({"activation_status": [{"this_host_uuid": NODE, "universe_uuid": R, "highest_epoch_seen": 2, "authority_serial": 0}]}),
        );
    }
}

impl Drop for Resident {
    fn drop(&mut self) {
        self.stop();
    }
}

fn write_one(evidence: &Path, input: &str, source: &str, at: i64, content: Value) {
    let file = json!({"form": EVIDENCE_FORM, "input": input, "source": source, "collected_at": at, "content": content});
    fs::write(
        evidence.join(format!("{input}.{source}.json")),
        file.to_string(),
    )
    .unwrap();
}

/// A field configuration that requires a hypervisor witness refuses even ledger initialization
/// when the live witness mount is missing.
#[test]
fn a_required_hypervisor_witness_refuses_every_vote_when_missing() {
    let mut resident = Resident::new(rustix::process::geteuid().as_raw());
    if !resident.mount_read_only() {
        eprintln!("SKIPPED: read-only mounts need unprivileged user namespaces");
        return;
    }
    resident
        .config
        .votes
        .as_mut()
        .unwrap()
        .require_generation_id = true;
    resident.start();
    assert_eq!(
        resident.ledger_state()["ledger_state"],
        "generation_identity_unreadable"
    );
    let result = resident
        .control(&json!({"operation": "vote_ledger_init", "operation_id": "init-no-generation"}));
    assert_eq!(result["code"], "generation_identity_unreadable", "{result}");
    resident.stop();
}

/// Proves, through the resident's control socket: only the operator creates, marks and readmits the
/// ledger; a missing and a new ledger sign nothing; readmission refuses what it cannot read, naming
/// it, then waits out the certificate life, then admits above the node's screen; a vote is signed and
/// recorded in `votes/r0` before it is answered; another holder for that epoch is refused, before and
/// after a restart of the process; a ledger put back from an older copy trips the wire on the
/// resident's own recorded votes and the status says so; a ledger on another host signs nothing; and
/// the resident's vote with replica-b's assembles into a certificate the node's rules accept.
#[test]
fn the_resident_signs_votes_under_its_ledger() {
    // The operator's operations refuse any other caller.
    let mut stranger = Resident::new(rustix::process::geteuid().as_raw() + 1);
    stranger.start();
    let refused =
        stranger.control(&json!({"operation": "vote_ledger_init", "operation_id": "init-1"}));
    assert_eq!(refused["code"], "caller_uid_refused", "{refused}");
    stranger.stop();

    // The machine-id on a writable mount: said at the start, and nothing is signed or created.
    let mut resident = Resident::new(rustix::process::geteuid().as_raw());
    resident.start();
    assert_eq!(
        resident.ledger_state()["ledger_state"],
        "host_identity_not_read_only"
    );
    assert!(resident
        .stderr()
        .contains("resident vote alert: host_identity_not_read_only"));
    assert_eq!(
        resident.sign("sign-rw", &takeover(3, X, now()))["code"],
        "host_identity_not_read_only"
    );
    let refused =
        resident.control(&json!({"operation": "vote_ledger_init", "operation_id": "init-rw"}));
    assert_eq!(refused["code"], "host_identity_not_read_only", "{refused}");
    resident.stop();
    if !resident.mount_read_only() {
        eprintln!(
            "SKIPPED the rest: read-only mounts need unprivileged user namespaces (unshare -rm)"
        );
        return;
    }
    resident.start();
    assert_eq!(resident.ledger_state()["ledger_state"], "ledger_missing");
    assert_eq!(
        resident.sign("sign-0", &takeover(3, X, now()))["code"],
        "ledger_missing"
    );
    let created =
        resident.control(&json!({"operation": "vote_ledger_init", "operation_id": "init-1"}));
    assert_eq!(created["vote_ledger"], "created", "{created}");
    assert_eq!(resident.ledger_state()["ledger_state"], "unadmitted");
    assert_eq!(
        resident.sign("sign-1", &takeover(3, X, now()))["code"],
        "ledger_unadmitted"
    );

    // Readmission: nothing to read yet, then too early, then admitted above the screen.
    let unreadable = resident.readmit("readmit-1", &json!({}));
    assert_eq!(
        unreadable["code"], "readmission_inputs_unreadable",
        "{unreadable}"
    );
    let named: Vec<String> = unreadable["unreadable"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| {
            format!(
                "{} {}",
                u["input"].as_str().unwrap(),
                u["source"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(
        named,
        [
            "ledger replica-b",
            "ledger replica-c",
            format!("screen {NODE}").as_str()
        ]
    );
    resident.write_evidence(now());
    let stated = resident.digests();
    let early = resident.readmit("readmit-2", &stated);
    assert_eq!(early["code"], "readmission_too_early", "{early}");
    let ready_at = early["retry_at"].as_i64().unwrap();
    while now() < ready_at {
        thread::sleep(Duration::from_millis(200));
    }
    // The screen replaced after its digest was stated: refused, by name.
    let screen = resident.evidence.join(format!("screen.{NODE}.json"));
    let original = fs::read(&screen).unwrap();
    write_one(
        &resident.evidence,
        "screen",
        NODE,
        now(),
        json!({"activation_status": []}),
    );
    let substituted = resident.readmit("readmit-3a", &stated);
    assert_eq!(
        substituted["code"], "readmission_evidence_mismatch",
        "{substituted}"
    );
    fs::write(&screen, original).unwrap();
    let admitted = resident.readmit("readmit-3", &stated);
    assert_eq!(admitted["vote_ledger"], "admitted", "{admitted}");
    assert_eq!(admitted["admission"]["floors_set"][R]["epoch_floor"], 2);
    assert_eq!(resident.ledger_state()["ledger_state"], "admitted");

    // Signing, recorded before it is answered.
    assert_eq!(
        resident.sign("sign-2", &takeover(2, X, now()))["code"],
        "epoch_at_or_below_floor"
    );
    let signed = resident.sign("sign-3", &takeover(3, X, now()));
    let first = signed["vote"].clone();
    assert!(
        signed["fact"].as_str().unwrap().starts_with("r0:"),
        "{signed}"
    );
    vote::verify_vote(&first, &policy()).unwrap();
    assert_eq!(
        resident.sign("sign-4", &takeover(3, Y, now()))["code"],
        "epoch_already_promised"
    );
    let after_first = fs::read(resident.signer(HOST).ledger_path()).unwrap();

    // Across a restart of the process.
    resident.stop();
    resident.start();
    assert_eq!(
        resident.sign("sign-5", &takeover(3, Y, now()))["code"],
        "epoch_already_promised"
    );
    let second = resident.sign("sign-6", &takeover(4, Y, now()));
    assert!(second["vote"].is_object(), "{second}");
    resident.stop();
    let inspection = inspect_read_only(
        &resident.config.network.database_path,
        &resident.config.network.manager,
        "r0",
    )
    .unwrap();
    let recorded: Vec<&podmesh_manager_ha_lab::Fact> = inspection
        .ordered_facts
        .iter()
        .filter(|f| f.scope == "votes/r0")
        .collect();
    assert_eq!(recorded.len(), 2);
    assert_eq!(
        serde_json::from_str::<Value>(&recorded[0].value).unwrap(),
        first
    );

    // The ledger put back from the copy taken after the first vote: the store holds the second.
    fs::write(resident.signer(HOST).ledger_path(), &after_first).unwrap();
    resident.start();
    let tripped = resident.sign("sign-7", &takeover(4, X, now()));
    assert_eq!(tripped["code"], "ledger_behind_own_votes", "{tripped}");
    let state = resident.ledger_state();
    assert_eq!(state["ledger_state"], "unadmitted");
    assert!(
        state["alerts"][0]
            .as_str()
            .unwrap()
            .starts_with("ledger_behind_own_votes"),
        "{state}"
    );
    assert!(resident
        .stderr()
        .contains("resident vote alert: ledger_behind_own_votes"));
    resident.stop();

    // The same directory seen from another host signs nothing.
    // Written on the host's side; the resident still reads it through the read-only mount.
    fs::write(&resident.host_file, format!("{OTHER_HOST}\n")).unwrap();
    resident.start();
    assert_eq!(
        resident.ledger_state()["ledger_state"],
        "ledger_foreign_host"
    );
    assert_eq!(
        resident.sign("sign-8", &takeover(5, X, now()))["code"],
        "ledger_foreign_host"
    );
    resident.stop();

    // The resident's first vote and replica-b's make the certificate.
    let payload = first["payload"].clone();
    assert_eq!(
        vote::assemble(&policy(), &payload, std::slice::from_ref(&first))
            .unwrap_err()
            .code,
        "below_threshold"
    );
    let vote_b = sign_outside_ledger(&payload, "replica-b", 2);
    let certificate = vote::assemble(&policy(), &payload, &[first, vote_b]).unwrap();
    assert_eq!(
        policy()
            .verify(&certificate, QUORUM_PROOF_KIND, TAKEOVER_FIELDS)
            .unwrap(),
        ["replica-a", "replica-b"]
    );
}

/// replica-b's signature over a payload, made here with its test key: only its payload signature
/// enters the certificate, and the assembly checks both of the vote's signatures.
fn sign_outside_ledger(payload: &Value, key_id: &str, seed: u8) -> Value {
    use ed25519_dalek::Signer as _;
    let key = SigningKey::from_bytes(&[seed; 32]);
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    let message = podmesh_manager_resident_lab::quorum::canonical_json(payload).unwrap();
    let mut v = json!({"form": vote::VOTE_FORM, "voter": key_id, "certificate_kind": payload["kind"], "payload": payload,
                       "signature": hex(&key.sign(&message).to_bytes()), "ledger_nonce": "00", "ledger_sequence": 1});
    let envelope = podmesh_manager_resident_lab::quorum::canonical_json(&v).unwrap();
    v["envelope_signature"] = json!(hex(&key.sign(&envelope).to_bytes()));
    v
}
