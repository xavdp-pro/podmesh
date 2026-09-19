//! The manager decides (V3-5), with compiled residents on loopback: a proposal made on one replica
//! reaches the others by replication, each checks it against its own view and votes under its own
//! ledger, and two votes of three make the certificate any replica reads; a proposal that breaks the
//! barrier is refused by every voter; a replica whose ledger is unadmitted does not vote, and one
//! vote decides nothing; a retried signature is answered with the recorded vote; and a peer that
//! forges votes over a live authenticated link has none of them counted.
//!
//! Test-only keys from a one-byte seed. The ledgers are admitted by the library's readmission with a
//! clock past its wait (the operator's procedure, run end to end with evidence, is the local
//! end-to-end test's, `tests/e2e/decisions-e2e.py`).
// Three replicas, indexed alike in every table here.
#![allow(clippy::needless_range_loop)]
use ed25519_dalek::SigningKey;
use podmesh_manager_ha_lab::{
    durable::{inspect_read_only, Configuration as Manager, Snapshot},
    Fact, ReplicaConfig, ScopeGrant,
};
use podmesh_manager_network_lab::{ConfigurationFile, Peer};
use podmesh_manager_resident_lab::{
    decisions::{proposal_scope, DecisionConfiguration, ResourceRules},
    ledger::{HostIdentity, Signer, SigningRules},
    quorum::{self, Quorum, QUORUM_PROOF_KIND, TAKEOVER_FIELDS},
    readmission::{self, Evidence, Scope},
    vote,
    votes::VoteConfiguration,
    Configuration,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const R: &str = "91eeb6bf-5489-405b-b77a-53105b0aff7a";
const NODES: [&str; 3] = [
    "0a0a0a0a-0000-4000-8000-000000000000",
    "1b1b1b1b-1111-4111-8111-111111111111",
    "2c2c2c2c-2222-4222-8222-222222222222",
];
const KEYS: [(&str, u8); 3] = [("replica-a", 1), ("replica-b", 2), ("replica-c", 3)];
const HOSTS: [&str; 3] = [
    "000000000000000000000000000000a0",
    "000000000000000000000000000000b1",
    "000000000000000000000000000000c2",
];
const BOOT: &str = "0b7f6f1e-6a55-4c1d-8f53-1c2d3e4f5a6b";
const REBOOT: &str = "5e6f7a8b-9c0d-4e1f-8a2b-3c4d5e6f7a8b";
const LIFE: i64 = 60;
const LEASE: i64 = 5;
const MARGIN: i64 = 5;
/// The deadline every vote operation must answer within (`VOTE_CONTROL_DEADLINE`), while the control
/// loop keeps answering everything else within its own 250 ms.
const VOTE_DEADLINE_MS: u64 = 2_000;

fn now() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap()
}

fn public(seed: u8) -> String {
    quorum::hex(
        &SigningKey::from_bytes(&[seed; 32])
            .verifying_key()
            .to_bytes(),
    )
}

fn quorum_value() -> Value {
    json!({"threshold": 2, "keys": KEYS.iter().map(|(id, s)| json!({"key_id": id, "public_key": public(*s)})).collect::<Vec<_>>()})
}

fn policy() -> Quorum {
    Quorum::declared("replicas", &quorum_value()).unwrap()
}

fn private(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn until(what: &str, timeout: Duration, mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !predicate() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(50));
    }
}

/// A takeover payload for the resource, live from now for the certificate life.
fn payload(
    epoch: i64,
    previous: Option<&str>,
    holder: &str,
    boot: &str,
    method: &str,
    eligible: i64,
) -> Value {
    let at = now();
    json!({"kind": QUORUM_PROOF_KIND, "authority_id": "replicas", "policy_digest": policy().digest(), "resource": R,
           "new_epoch": epoch, "previous_epoch": epoch - 1, "new_holder": holder, "previous_holder": previous,
           "holder_boot_id": boot, "grant_id": format!("g{epoch}-{at}"), "method": method,
           "eligible_after": eligible, "issued_at": at, "expires_at": at + LIFE})
}

/// A read-only bind mount of `paths`, held by a helper in its own user and mount namespace and
/// reached through `/proc/<helper>/root`, as `tests/votes.rs` does.
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
        script.push_str("echo ready && exec sleep 900");
        let mut helper = Command::new("unshare")
            .args(["-rm", "sh", "-c", &script])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut line = String::new();
        BufReader::new(helper.stdout.take()?)
            .read_line(&mut line)
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

/// Three addresses on a loopback network nobody else holds, reserved until each resident binds its.
fn loopback_addresses() -> (Vec<SocketAddr>, Vec<Option<TcpListener>>, TcpListener) {
    let seed =
        std::process::id().wrapping_mul(2_654_435_761) ^ u32::try_from(now() & 0xffff).unwrap();
    for attempt in 0..4_096_u32 {
        let candidate = seed.wrapping_add(attempt.wrapping_mul(7_919));
        let b = u8::try_from(candidate / 251 % 251).unwrap() + 2;
        let c = u8::try_from(candidate % 251).unwrap() + 2;
        let port = 10_000 + u16::try_from(candidate % 10_000).unwrap();
        let address = |last: u8| SocketAddr::from((std::net::Ipv4Addr::new(127, b, c, last), port));
        let Ok(claim) = TcpListener::bind(address(1)) else {
            continue;
        };
        let reserved: Vec<_> = (0..3).map(|i| TcpListener::bind(address(10 + i))).collect();
        if reserved.iter().any(Result::is_err) {
            continue;
        }
        return (
            (0..3).map(|i| address(10 + i)).collect(),
            reserved.into_iter().map(Result::ok).collect(),
            claim,
        );
    }
    panic!("no free private loopback network");
}

fn pair_key(i: usize, j: usize) -> String {
    format!("{:02x}", 0x40 + i.min(j) * 3 + i.max(j)).repeat(32)
}

struct Lab {
    dir: tempfile::TempDir,
    configs: Vec<Configuration>,
    vote_dirs: Vec<PathBuf>,
    host_views: Vec<PathBuf>,
    children: Vec<Option<Child>>,
    reserved: Vec<Option<TcpListener>>,
    _claim: TcpListener,
    _mounts: ReadOnlyMounts,
}

impl Lab {
    /// Three replicas, each with its key, an admitted ledger bound to its own host, its machine-id and
    /// evidence directory mounted read-only, deciding the resource for three nodes.
    fn new() -> Option<Self> {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let (addresses, reserved, claim) = loopback_addresses();
        let manager = Manager {
            logical_manager_id: "decisions-test".into(),
            replicas: (0..3)
                .map(|i| ReplicaConfig {
                    replica_id: format!("r{i}"),
                    host_id: format!("h{i}"),
                })
                .collect(),
            grants: (0..3)
                .flat_map(|i| {
                    [
                        vote::vote_scope(&format!("r{i}")),
                        proposal_scope(&format!("r{i}")),
                    ]
                    .into_iter()
                    .map(move |scope| ScopeGrant {
                        scope,
                        owner_replica_id: format!("r{i}"),
                    })
                })
                .collect(),
        };
        let replica_keys: BTreeMap<String, String> = (0..3)
            .map(|i| (format!("r{i}"), KEYS[i].0.to_string()))
            .collect();
        let mut host_files = Vec::new();
        let mut evidence = Vec::new();
        for i in 0..3 {
            let host = dir.path().join(format!("machine-id-{i}"));
            fs::write(&host, format!("{}\n", HOSTS[i])).unwrap();
            host_files.push(host);
            let e = dir.path().join(format!("evidence-{i}"));
            private(&e);
            evidence.push(e);
        }
        let paths: Vec<&Path> = host_files
            .iter()
            .chain(evidence.iter())
            .map(PathBuf::as_path)
            .collect();
        let mounts = ReadOnlyMounts::new(&paths)?;
        let uid = rustix::process::geteuid().as_raw();
        let mut configs = Vec::new();
        let mut vote_dirs = Vec::new();
        for i in 0..3 {
            let vote_dir = dir.path().join(format!("votes-{i}"));
            private(&vote_dir);
            let key = vote_dir.join(format!("{}.key", KEYS[i].0));
            fs::write(&key, format!("{}\n", quorum::hex(&[KEYS[i].1; 32]))).unwrap();
            fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
            // The ledger, created and admitted by the library's readmission with a clock past its wait.
            let signer = Signer::open(
                &vote_dir,
                KEYS[i].0,
                HostIdentity::from_machine_id(HOSTS[i]).unwrap(),
                policy(),
                SigningRules {
                    max_certificate_life: LIFE,
                    lock_wait: Duration::from_secs(2),
                },
            )
            .unwrap();
            signer.init(now()).unwrap();
            let nodes: Vec<String> = NODES.iter().map(|n| (*n).to_string()).collect();
            let replica_id = format!("r{i}");
            let scope = Scope {
                manager: &manager,
                replica_id: &replica_id,
                replica_keys: &replica_keys,
                known_keys: vote::keys_of(&policy()),
                policy_keys: KEYS.iter().map(|k| k.0.to_string()).collect(),
                retired_keys: Default::default(),
                nodes: &nodes,
            };
            readmission::readmit(
                &signer,
                &scope,
                Evidence::default(),
                "test-admission",
                None,
                now() + 100_000,
            )
            .unwrap();
            vote_dirs.push(vote_dir);
            configs.push(Configuration {
                network: ConfigurationFile {
                    replica_id: format!("r{i}"),
                    database_path: dir.path().join(format!("r{i}.sqlite")),
                    manager: manager.clone(),
                    bind: addresses[i],
                    peers: (0..3)
                        .filter(|j| *j != i)
                        .map(|j| Peer {
                            replica_id: format!("r{j}"),
                            endpoint: addresses[j],
                            shared_key_hex: pair_key(i, j),
                        })
                        .collect(),
                },
                control_socket: dir.path().join(format!("r{i}.sock")),
                observation_writer_uid: uid,
                interval_ms: 100,
                max_backoff_ms: 400,
                incoming_workers: 2,
                full_verification_interval_ms: None,
                unchanged_snapshot_refresh_ms: None,
                catch_up_window_ms: Some(1_000),
                votes: Some(VoteConfiguration {
                    key_id: KEYS[i].0.into(),
                    authority_id: "replicas".into(),
                    authority_quorum: quorum_value(),
                    authority_serial: 0,
                    replica_keys: replica_keys.clone(),
                    retired_keys: vec![],
                    nodes: NODES.iter().map(|n| (*n).to_string()).collect(),
                    evidence_dir: mounts.view(&evidence[i]),
                    operator_uid: uid,
                    max_certificate_life_seconds: LIFE,
                    decisions: Some(DecisionConfiguration {
                        voter_interval_ms: 200,
                        resources: vec![ResourceRules {
                            resource: R.into(),
                            lease_seconds: LEASE,
                            takeover_margin_seconds: MARGIN,
                            renewal_not_after: 0,
                            baseline: None,
                        }],
                    }),
                }),
            });
        }
        let host_views = host_files.iter().map(|h| mounts.view(h)).collect();
        Some(Self {
            dir,
            configs,
            vote_dirs,
            host_views,
            children: vec![None, None, None],
            reserved,
            _claim: claim,
            _mounts: mounts,
        })
    }

    fn log(&self, i: usize) -> PathBuf {
        self.dir.path().join(format!("r{i}.stderr"))
    }

    fn start(&mut self, i: usize) {
        drop(self.reserved[i].take());
        let path = self.dir.path().join(format!("r{i}.json"));
        fs::write(&path, serde_json::to_vec(&self.configs[i]).unwrap()).unwrap();
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log(i))
            .unwrap();
        self.children[i] = Some(
            Command::new(env!("CARGO_BIN_EXE_podmesh-manager-resident-lab"))
                .env("PODMESH_MANAGER_NETWORK_MODE", "authenticated-static-peers")
                .env("PODMESH_MANAGER_VOTE_DIR", &self.vote_dirs[i])
                .env("PODMESH_MANAGER_HOST_ID_FILE", &self.host_views[i])
                .arg(path)
                .stdout(Stdio::null())
                .stderr(Stdio::from(log))
                .spawn()
                .unwrap(),
        );
        until(&format!("r{i} answers"), Duration::from_secs(20), || {
            self.try_control(i, &json!({"operation": "status"}))
                .is_some()
        });
    }

    fn stop(&mut self, i: usize) {
        if let Some(mut child) = self.children[i].take() {
            let _ = self.try_control(i, &json!({"operation": "shutdown"}));
            let deadline = Instant::now() + Duration::from_secs(10);
            while child.try_wait().unwrap().is_none() {
                if Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = fs::remove_file(&self.configs[i].control_socket);
                    return;
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
    }

    fn try_control(&self, i: usize, request: &Value) -> Option<Value> {
        let mut stream = UnixStream::connect(&self.configs[i].control_socket).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        stream.write_all(request.to_string().as_bytes()).ok()?;
        stream.shutdown(Shutdown::Write).ok()?;
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// A control request, asked again while the answer is uncertain, busy or catching up.
    fn control(&self, i: usize, request: &Value) -> Value {
        for _ in 0..100 {
            let answer = self
                .try_control(i, request)
                .expect("the control socket answers");
            if !matches!(
                answer["error"].as_str(),
                Some(
                    "vote_operation_uncertain"
                        | "vote_busy"
                        | "vote_catching_up"
                        | "proposal_catching_up"
                )
            ) {
                return answer;
            }
            thread::sleep(Duration::from_millis(100));
        }
        panic!("no certain answer to {request}: {}", self.status(i));
    }

    fn propose(&self, i: usize, id: &str, payload: &Value) -> Value {
        let answer = self.control(
            i,
            &json!({"operation": "decision_propose", "operation_id": id, "payload": payload}),
        );
        assert!(answer["proposal"].is_string(), "{answer}");
        answer
    }

    fn read(&self, i: usize) -> Value {
        let answer = self.control(i, &json!({"operation": "decision_read", "resource": R}));
        assert!(answer["decision"].is_object(), "{answer}");
        answer["decision"].clone()
    }

    fn pending(&self, i: usize, digest: &str) -> Option<Value> {
        self.read(i)["pending"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["payload_digest"] == digest)
            .cloned()
    }

    /// Waits until replica `i` reads epoch `epoch` as decided, and returns its certificate.
    fn decided(&self, i: usize, epoch: i64) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let last = self.read(i);
            if last["current"]["epoch"] == epoch {
                return last["current"]["certificate"].clone();
            }
            assert!(
                Instant::now() < deadline,
                "r{i} never read epoch {epoch} decided: {last}"
            );
            thread::sleep(Duration::from_millis(100));
        }
    }

    fn status(&self, i: usize) -> Value {
        self.try_control(i, &json!({"operation": "status"}))
            .expect("the control socket answers")
    }

    fn inspection(&self, i: usize) -> Vec<Fact> {
        let c = &self.configs[i].network;
        inspect_read_only(&c.database_path, &c.manager, &c.replica_id)
            .unwrap()
            .ordered_facts
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        for i in 0..3 {
            self.stop(i);
        }
    }
}

/// The node's rules applied to a certificate: its signers, once the port of the node's verifier
/// (pinned to the node's own vectors) accepts it.
fn signers(certificate: &Value) -> Vec<String> {
    policy()
        .verify(certificate, QUORUM_PROOF_KIND, TAKEOVER_FIELDS)
        .unwrap()
}

fn digest(payload: &Value) -> String {
    quorum::payload_digest(payload).unwrap()
}

/// Proves, with three compiled residents: an epoch rotation and a same-holder re-issue decided by
/// two replicas of three, each certificate read from a replica that did not propose it and accepted
/// by the node's rules; a proposal that breaks the barrier refused by every voter, by name, never
/// decided, and refused through `vote_sign` too; a replica whose ledger is unadmitted does not vote,
/// the two others decide; with one of them stopped, one vote decides nothing and the read says one
/// is missing; a signature retried on a decided payload is answered with the vote already recorded,
/// appending nothing; and every vote operation answered within the vote deadline, the timings
/// printed.
#[test]
fn two_replicas_of_three_decide_and_one_does_not() {
    let Some(mut lab) = Lab::new() else {
        eprintln!("SKIPPED: read-only mounts need unprivileged user namespaces (unshare -rm)");
        return;
    };
    for i in 0..3 {
        lab.start(i);
    }
    for i in 0..3 {
        until(&format!("r{i} caught up"), Duration::from_secs(20), || {
            lab.status(i)["catch_up"]["caught_up"] == true
        });
    }
    // Epoch 1, the first, proposed on r0 and read on r2.
    let first = payload(1, None, NODES[0], BOOT, "first", now());
    let answer = lab.propose(0, "propose-1", &first);
    assert_eq!(answer["here"]["verdict"], "passes", "{answer}");
    let c1 = lab.decided(2, 1);
    assert!(signers(&c1).len() >= 2);
    assert_eq!(quorum::payload_digest(&c1).unwrap(), digest(&first));

    // A same-holder re-issue after a reboot of the holder: epoch 2 for the new boot, the barrier carried.
    let reissue = payload(
        2,
        Some(NODES[0]),
        NODES[0],
        REBOOT,
        "same_holder",
        first["eligible_after"].as_i64().unwrap(),
    );
    lab.propose(1, "propose-2", &reissue);
    let c2 = lab.decided(0, 2);
    assert_eq!(
        (c2["holder_boot_id"].as_str(), signers(&c2).len() >= 2),
        (Some(REBOOT), true)
    );

    // A rotation to another holder whose barrier does not cover the current certificate's expiry:
    // every voter refuses it, and it is never decided.
    let early = payload(
        3,
        Some(NODES[0]),
        NODES[1],
        BOOT,
        "lease_barrier",
        now() + LEASE + MARGIN,
    );
    lab.propose(2, "propose-3-early", &early);
    for i in 0..3 {
        until(
            &format!("r{i} refuses the early barrier"),
            Duration::from_secs(10),
            || {
                lab.pending(i, &digest(&early))
                    .is_some_and(|p| p["here"]["code"] == "barrier_too_early" && p["votes"] == 0)
            },
        );
    }
    // Asked directly, through `vote_sign`, a replica checks the same rules before it signs.
    let direct = lab.control(
        0,
        &json!({"operation": "vote_sign", "operation_id": "direct-early", "payload": early}),
    );
    assert_eq!(direct["code"], "barrier_too_early", "{direct}");
    // The rotation with the barrier the view requires: epoch 2's expiry plus the lease and the margin.
    let required = c2["expires_at"].as_i64().unwrap() + LEASE + MARGIN;
    let mut rotation = payload(3, Some(NODES[0]), NODES[1], BOOT, "lease_barrier", required);
    // The barrier must fall within the certificate's life.
    rotation["expires_at"] = json!(required.max(rotation["expires_at"].as_i64().unwrap()));
    rotation["issued_at"] = json!(rotation["expires_at"].as_i64().unwrap() - LIFE);
    lab.propose(1, "propose-3", &rotation);
    let c3 = lab.decided(2, 3);
    assert_eq!(
        (c3["new_holder"].as_str(), c3["previous_holder"].as_str()),
        (Some(NODES[1]), Some(NODES[0]))
    );
    lab.decided(0, 3);
    assert_eq!(
        lab.pending(0, &digest(&early)),
        None,
        "the early proposal is below the decided epoch now"
    );

    // A signature retried on a decided payload: the recorded vote again, nothing appended.
    let again = |id: &str| {
        lab.control(
            0,
            &json!({"operation": "vote_sign", "operation_id": id, "payload": rotation}),
        )
    };
    let (a, b) = (again("retry-1"), again("retry-2"));
    assert_eq!(
        (a["replayed"].as_bool(), b["replayed"].as_bool()),
        (Some(true), Some(true)),
        "{a} {b}"
    );
    assert_eq!(
        (a["vote"].clone(), a["fact"].clone()),
        (b["vote"].clone(), b["fact"].clone())
    );
    let own_votes_before = lab.status(0)["votes"]["sequence"].clone();

    // r2's ledger marked unadmitted: it does not vote, and r0 and r1 decide epoch 4 without it.
    let marked = lab.control(2, &json!({"operation": "vote_ledger_mark_unadmitted", "operation_id": "mark-1", "reason": "test"}));
    assert_eq!(marked["vote_ledger"], "unadmitted", "{marked}");
    let fourth = payload(4, Some(NODES[1]), NODES[1], BOOT, "same_holder", required);
    let mut fourth = fourth;
    fourth["expires_at"] = rotation["expires_at"].clone();
    fourth["issued_at"] = rotation["issued_at"].clone();
    lab.propose(2, "propose-4", &fourth);
    let c4 = lab.decided(2, 4);
    let who = signers(&c4);
    assert!(
        !who.contains(&"replica-c".to_string()),
        "the unadmitted replica voted: {who:?}"
    );
    until("r2's verdict on epoch 4", Duration::from_secs(5), || {
        lab.status(2)["votes"]["ledger_state"] == "unadmitted"
    });
    assert_eq!(
        lab.status(0)["votes"]["sequence"].as_u64(),
        own_votes_before.as_u64().map(|s| s + 1)
    );

    // r1 stopped: r0's vote alone decides nothing, and the read says one is missing.
    lab.stop(1);
    let mut fifth = payload(5, Some(NODES[1]), NODES[1], BOOT, "same_holder", required);
    fifth["expires_at"] = rotation["expires_at"].clone();
    fifth["issued_at"] = rotation["issued_at"].clone();
    lab.propose(0, "propose-5", &fifth);
    until("r0 votes for epoch 5", Duration::from_secs(10), || {
        lab.pending(0, &digest(&fifth)).is_some_and(|p| {
            p["votes"] == 1 && p["missing"] == 1 && p["here"]["verdict"] == "voted"
        })
    });
    until("r2 refuses epoch 5", Duration::from_secs(10), || {
        lab.pending(2, &digest(&fifth))
            .is_some_and(|p| p["here"]["code"] == "ledger_unadmitted")
    });
    thread::sleep(Duration::from_secs(2));
    for i in [0, 2] {
        assert_eq!(
            lab.read(i)["current"]["epoch"],
            4,
            "one vote decided epoch 5 on r{i}"
        );
    }

    // Every vote operation answered within the vote deadline, and the voter's passes measured.
    for i in [0, 2] {
        let status = lab.status(i);
        for (op, t) in status["votes"]["operations"].as_object().unwrap() {
            assert!(
                t["longest_ms"].as_u64().unwrap() < VOTE_DEADLINE_MS,
                "r{i} {op}: {t}"
            );
        }
        eprintln!(
            "r{i} timings: {} voter: {}",
            status["votes"]["operations"], status["votes"]["voter"]
        );
    }
    for i in 0..3 {
        lab.stop(i);
    }
    // The retries appended nothing: one vote of r0's for epoch 3.
    let facts = lab.inspection(0);
    let own_epoch_3 = facts
        .iter()
        .filter(|f| f.scope == "votes/r0" && f.subject == format!("epoch:{R}:3"))
        .count();
    assert_eq!(own_epoch_3, 1);
}

/// One authenticated push of `facts` from r2 to replica `to`, over their link's pair key, as the
/// transport frames it: what a compromised peer can send.
fn push_as_r2(lab: &Lab, to: usize, facts: Vec<Fact>) -> Value {
    use hmac::{Hmac, Mac};
    let manager = lab.configs[to].network.manager.clone();
    let snapshot = Snapshot {
        configuration: manager,
        replica_id: "r2".into(),
        facts,
    };
    let protocol = "podmesh-manager-network-lab/1";
    let (op, nonce) = (format!("forge-{}", now()), format!("nonce-{}", now()));
    let destination = format!("r{to}");
    let bytes = serde_json::to_vec(&(
        protocol,
        "r2",
        destination.as_str(),
        op.as_str(),
        nonce.as_str(),
        &snapshot,
    ))
    .unwrap();
    let key: Vec<u8> = (0..32)
        .map(|k| u8::from_str_radix(&pair_key(2, to)[2 * k..2 * k + 2], 16).unwrap())
        .collect();
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(&key).unwrap();
    mac.update(b"podmesh-manager-network-lab/1\0");
    mac.update(&bytes);
    let mac_hex = quorum::hex(&mac.finalize().into_bytes());
    let body = serde_json::to_vec(&json!({"protocol": protocol, "source_replica_id": "r2", "destination_replica_id": destination,
                                          "operation_id": op, "nonce": nonce, "snapshot": snapshot, "mac_hex": mac_hex}))
    .unwrap();
    let mut stream = TcpStream::connect(lab.configs[to].network.bind).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .write_all(&u32::try_from(body.len()).unwrap().to_be_bytes())
        .unwrap();
    stream.write_all(&body).unwrap();
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length).unwrap();
    let mut reply = vec![0; u32::from_be_bytes(length) as usize];
    stream.read_exact(&mut reply).unwrap();
    serde_json::from_slice(&reply).unwrap()
}

/// A fact as a replica's store would hold it.
fn fact(origin: usize, sequence: u64, scope: &str, subject: &str, value: String) -> Fact {
    Fact {
        event_id: format!("r{origin}:{sequence:020}"),
        logical_manager_id: "decisions-test".into(),
        origin_replica_id: format!("r{origin}"),
        origin_host_id: format!("h{origin}"),
        producer_sequence: sequence,
        scope: scope.into(),
        subject: subject.into(),
        subject_revision: 1,
        predecessor: None,
        exclusive_resource: None,
        active_claim: false,
        value,
    }
}

/// A vote made with `seed`'s key in `voter`'s name: genuine when they match, forged otherwise.
fn vote_by(payload: &Value, voter: &str, seed: u8) -> Value {
    use ed25519_dalek::Signer as _;
    let key = SigningKey::from_bytes(&[seed; 32]);
    let message = quorum::canonical_json(payload).unwrap();
    let mut v = json!({"form": vote::VOTE_FORM, "voter": voter, "certificate_kind": payload["kind"], "payload": payload,
                       "signature": quorum::hex(&key.sign(&message).to_bytes()), "ledger_nonce": "00", "ledger_sequence": 1});
    let envelope = quorum::canonical_json(&v).unwrap();
    v["envelope_signature"] = json!(quorum::hex(&key.sign(&envelope).to_bytes()));
    v
}

/// Proves, over a live authenticated link: replica r2, compromised and holding its own key and its
/// link's pair key, pushes to r1 a proposal the honest replicas refuse, its own genuine vote for it,
/// a vote in replica-a's name signed by its own key in its own scope, and a fact it attributes to r0
/// carrying another vote in replica-a's name. r1 imports them (the link is authenticated), and
/// counts one vote: r2's own. Nothing is decided, and the read names one vote missing.
#[test]
fn a_peer_forging_votes_over_a_live_link_has_none_counted() {
    let Some(mut lab) = Lab::new() else {
        eprintln!("SKIPPED: read-only mounts need unprivileged user namespaces (unshare -rm)");
        return;
    };
    // The three residents catch up with one another (a new store appends nothing before it has),
    // then r2's resident stops: from here r2 is the forger, speaking on its own link.
    for i in 0..3 {
        lab.start(i);
    }
    for i in 0..3 {
        until(&format!("r{i} caught up"), Duration::from_secs(30), || {
            lab.status(i)["catch_up"]["caught_up"] == true
        });
    }
    lab.stop(2);
    // A proposal the honest replicas refuse: a lease barrier of epoch 1 far in the future is fine,
    // an unknown holder is not.
    let bad = payload(
        1,
        None,
        "3d3d3d3d-3333-4333-8333-333333333333",
        BOOT,
        "first",
        now(),
    );
    let proposal =
        serde_json::to_string(&json!({"form": "podmesh-manager-proposal/1", "payload": bad}))
            .unwrap();
    let genuine = vote_by(&bad, "replica-c", 3);
    let in_a_name = vote_by(&bad, "replica-a", 3);
    let facts = vec![
        fact(2, 1, "proposals/r2", &format!("proposal:{R}:1"), proposal),
        fact(
            2,
            2,
            "votes/r2",
            &format!("epoch:{R}:1"),
            genuine.to_string(),
        ),
        fact(2, 3, "votes/r2", "epoch:forged:a", in_a_name.to_string()),
        fact(
            0,
            1_000_000,
            "votes/r0",
            "epoch:forged:r0",
            in_a_name.to_string(),
        ),
    ];
    let reply = push_as_r2(&lab, 1, facts);
    assert_eq!(reply["result"], "imported", "{reply}");
    until(
        "r1 reads the forged proposal",
        Duration::from_secs(10),
        || lab.pending(1, &digest(&bad)).is_some(),
    );
    until("r1 refuses it", Duration::from_secs(10), || {
        lab.pending(1, &digest(&bad))
            .is_some_and(|p| p["here"]["code"] == "holder_not_a_node")
    });
    let seen = lab.pending(1, &digest(&bad)).unwrap();
    assert_eq!(
        (seen["votes"].as_u64(), seen["missing"].as_u64()),
        (Some(1), Some(1)),
        "{seen}"
    );
    assert_eq!(seen["voters"], json!(["replica-c"]));
    // Replication carries the same facts to r0, which counts the same.
    until("r0 reads it too", Duration::from_secs(15), || {
        lab.pending(0, &digest(&bad)).is_some()
    });
    thread::sleep(Duration::from_secs(1));
    for i in [0, 1] {
        let read = lab.read(i);
        assert!(
            read["current"].is_null(),
            "r{i} decided a forged proposal: {read}"
        );
        let p = lab.pending(i, &digest(&bad)).unwrap();
        assert_eq!(p["voters"], json!(["replica-c"]), "r{i}: {p}");
    }
}
