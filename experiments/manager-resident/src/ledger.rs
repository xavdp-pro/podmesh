//! The signing ledger (V3-4, rule R4 of the quorum model: the promise lives with the key).
//!
//! A replica's signing key is a secret its host mounts outside the universe's state (the operator's
//! custody decision). The ledger is one small file beside it, in the same host-provided directory,
//! `<vote_dir>/<key_id>.ledger`: never in the universe's state or store, so no recovery point, restore,
//! clone or migration of the replica's universe carries or rewinds it (assumption A1). It records every
//! promise the key made, per resource:
//!
//! - one entry per (resource, epoch), with the holder, for takeover certificates (an epoch rotation or
//!   a same-holder re-issue);
//! - one entry per (resource, from_serial), with the new policy's digest, for policy changes.
//!
//! Neither is keyed on the policy digest: a change of the authority set, the same keys at a new serial
//! or a re-keyed replica, never reopens an epoch or a serial this ledger promised (review finding 2).
//! Every promise carries the ledger's monotonic sequence number, which every vote carries too.
//!
//! The header binds the ledger to its key (`key_id` and public key), to the host (its machine-id, read
//! from a file the host mounts read-only, not from the universe) and to a random nonce made when the
//! ledger was created. A ledger that is missing, unreadable, made for another key or another host, or
//! marked unadmitted signs nothing and says why, by name (`LedgerRefusal::code`). A new ledger is
//! created unadmitted: only the operator's readmission (`readmission.rs`) admits one, after reading
//! everything that could show what its key promised before.
//!
//! Signing is one step under an exclusive `flock` on the vote directory itself (one signer per
//! ledger, A4; a lock file could be removed or replaced under a live signer): read and check the
//! ledger, check the tripwire, check the promise rule, write the new ledger to a temporary file,
//! fsync it, rename it over the ledger, fsync the directory (A2), and only then sign and return the
//! vote (A7). A crash anywhere before the end releases no signature.
//!
//! The tripwire (review finding 3): before signing, the signer reads the votes it is shown (its peers'
//! facts, and its own store's). A vote of its own key, whose signatures verify under its key, with a
//! sequence number above the ledger's, or one issued since the ledger's last admission that the ledger
//! does not hold, proves the ledger went back in time. The signer then marks the ledger unadmitted,
//! durably, refuses, and names the alert. It closes the easiest hole, a silent revert of a host's VM
//! snapshot, whenever one of the key's later votes survives anywhere the signer is shown.
//!
//! The watched generation witness (V3-5 review): `guard_generation` reads the hypervisor's value
//! before the resident exchanges anything, and releases the ledger's lock again. A snapshot resume
//! between that check and a signature takes everything inside the guest back with it -- this
//! process's memory, the ledger, the `<key_id>.generation-id` marker -- so a signature released
//! after it would leave a ledger that does not know it was made. Only the witness itself, which the
//! hypervisor holds outside the snapshot, still says which generation this is. A signer given that
//! file (`watch_generation`) therefore reads it again under the signing lock, in the last moment
//! before a vote is sealed, and readmission reads it again before it admits a ledger: a value other
//! than the one guarded marks the ledger unadmitted, durably, and releases nothing.
use crate::quorum::{self, Quorum};
use crate::vote::{self, Decision, LedgerStamp, PromiseKind};
use ed25519_dalek::{SigningKey, VerifyingKey};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// The ledger file's form.
pub const LEDGER_FORM: &str = "podmesh-manager-vote-ledger/1";
/// How far ahead of this replica's clock a payload may be issued, as the node allows.
pub const CLOCK_SKEW_SECONDS: i64 = 30;
/// The bound on the skew between any two clocks that matter here, a signer's and a node's: the
/// node's 30-second allowance on each side. Readmission waits the longest certificate life plus
/// this bound (review of V3-4, finding 5).
pub const CLOCK_SKEW_BOUND_SECONDS: i64 = 2 * CLOCK_SKEW_SECONDS;
/// The largest ledger file read.
const LEDGER_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Why the ledger signed nothing, or refused an operation, by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerRefusal {
    pub code: &'static str,
    pub detail: String,
}

impl std::fmt::Display for LedgerRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "signing refused ({}): {}", self.code, self.detail)
    }
}

impl std::error::Error for LedgerRefusal {}

pub(crate) fn refusal(code: &'static str, detail: impl Into<String>) -> LedgerRefusal {
    LedgerRefusal {
        code,
        detail: detail.into(),
    }
}

/// The code a watched generation witness raises when it no longer holds the value guarded: the
/// guest moved to another generation under a signature or an admission.
pub const GENERATION_CHANGED: &str = "generation_changed";

/// The code a signer raises when nothing has said whether it has a generation witness. Silence is
/// not a waiver: a replica that may sign either watches a witness, or carries the operator's
/// recorded decision to do without one. It never signs because no one mentioned the subject.
pub const GENERATION_WITNESS_ABSENT: &str = "generation_witness_absent";

/// The code a signer raises when the witness it was offered does not live outside the guest's
/// snapshot. A file on an ordinary filesystem goes back with the guest, and so does the page cache
/// that served its last read, so reading it again proves nothing about the generation.
pub const GENERATION_WITNESS_UNTRUSTED: &str = "generation_witness_untrusted";

/// `sysfs`, where the platform exposes QEMU's `fw_cfg` items. The hypervisor answers each read, and
/// a bind mount of such a file keeps its filesystem, so a mount into a universe still qualifies.
const SYSFS_MAGIC: rustix::fs::FsWord = 0x6265_6572;

/// The codes that mark the ledger unadmitted while it is signing: the tripwire's, and the watched
/// generation witness's. Each one is also an alert, named on standard error by the resident.
pub const TRIPWIRE_CODES: &[&str] = &[
    "ledger_behind_own_votes",
    "own_vote_unknown_to_ledger",
    GENERATION_CHANGED,
];

/// Says whether a path can be a generation witness at all, without reading it and without touching
/// the ledger. Call it **before** `guard_generation`, which persists a marker and can mark the
/// ledger unadmitted: a path that could never be a witness must not be able to quarantine anything.
///
/// # Errors
/// `generation_witness_untrusted`: the path cannot be read as a file, or is not on a filesystem the
/// hypervisor answers for. Only sysfs qualifies, where the platform exposes `fw_cfg` items; a bind
/// mount of such a file into a universe keeps that filesystem and still qualifies.
pub fn trust_generation_witness(path: &std::path::Path) -> Result<WitnessIdentity, LedgerRefusal> {
    let filesystem = rustix::fs::statfs(path)
        .map_err(|e| refusal(GENERATION_WITNESS_UNTRUSTED, e.to_string()))?;
    if filesystem.f_type != SYSFS_MAGIC {
        return Err(refusal(
            GENERATION_WITNESS_UNTRUSTED,
            format!(
                "the witness at {} is on filesystem type {:#x}, not the sysfs the hypervisor answers for ({SYSFS_MAGIC:#x}): a rollback would restore this file and the page cache that last read it, so reading it again would show the generation the guest went back to",
                path.display(),
                filesystem.f_type
            ),
        ));
    }
    current_witness_identity(path)
}

/// What a witness is right now: its filesystem, device and inode. This judges nothing — it only
/// says what is there, so that a later read can tell the file changed underneath it.
fn current_witness_identity(path: &std::path::Path) -> Result<WitnessIdentity, LedgerRefusal> {
    let filesystem = rustix::fs::statfs(path)
        .map_err(|e| refusal(GENERATION_WITNESS_UNTRUSTED, e.to_string()))?;
    let file = rustix::fs::stat(path)
        .map_err(|e| refusal(GENERATION_WITNESS_UNTRUSTED, e.to_string()))?;
    Ok(WitnessIdentity {
        filesystem: filesystem.f_type,
        device: file.st_dev,
        inode: file.st_ino,
    })
}

/// The host this replica runs on, as the host itself names it: its machine-id, 32 lowercase hex
/// characters, read from a file the host mounts read-only into the universe (by default
/// `/etc/machine-id`). A universe's own `/etc/machine-id` is not the host's and travels with a clone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostIdentity(String);

impl HostIdentity {
    /// # Errors
    /// `host_identity_unreadable`: not 32 lowercase hex characters.
    pub fn from_machine_id(text: &str) -> Result<HostIdentity, LedgerRefusal> {
        let id = text.trim_end_matches('\n');
        if id.len() != 32
            || !id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(refusal(
                "host_identity_unreadable",
                "a machine-id is 32 lowercase hex characters",
            ));
        }
        Ok(HostIdentity(id.to_string()))
    }

    /// # Errors
    /// `host_identity_unreadable`.
    /// Reads the host's machine-id from `path`, which must be a regular file, not a symlink, on a
    /// mount that is read-only for this process (`statvfs` `ST_RDONLY` on the opened file): the host
    /// mounts its identity into the universe read-only, and a replica that could write the file
    /// could make any ledger its own. This does not tell a whole-VM clone from its original, which
    /// carries the same machine-id: the rule of no VM clone of a laboratory host on the managed
    /// network (A3) stays the operator's.
    ///
    /// # Errors
    /// `host_identity_unreadable`, `host_identity_not_read_only`.
    pub fn read(path: &Path) -> Result<HostIdentity, LedgerRefusal> {
        use rustix::fs::{fstatvfs, open, Mode, OFlags, StatVfsMountFlags};
        let fail = |e: &dyn std::fmt::Display| {
            refusal(
                "host_identity_unreadable",
                format!("{}: {e}", path.display()),
            )
        };
        if !path.is_absolute() {
            return Err(fail(
                &"the host's machine-id file must be named by an absolute path",
            ));
        }
        let fd = open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NOCTTY,
            Mode::empty(),
        )
        .map_err(|e| fail(&e))?;
        let mut file = File::from(fd);
        if !file.metadata().map_err(|e| fail(&e))?.is_file() {
            return Err(fail(&"not a regular file"));
        }
        let mount = fstatvfs(&file).map_err(|e| fail(&e))?;
        if !mount.f_flag.contains(StatVfsMountFlags::RDONLY) {
            return Err(refusal(
                "host_identity_not_read_only",
                format!(
                    "{} is on a writable mount: the host's machine-id must be mounted read-only into the universe",
                    path.display()
                ),
            ));
        }
        let mut text = String::new();
        (&mut file)
            .take(64)
            .read_to_string(&mut text)
            .map_err(|e| fail(&e))?;
        HostIdentity::from_machine_id(&text)
    }

    #[must_use]
    pub fn machine_id(&self) -> &str {
        &self.0
    }
}

/// One promise: the decision a key signed for one (resource, epoch) or (resource, from_serial).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Promise {
    /// `new_holder` of an epoch, `new_policy_digest` of a serial.
    pub holder: String,
    /// The decision without its life: a re-issue with a fresh life keeps it.
    pub decision_digest: String,
    /// The latest document signed for it.
    pub payload_digest: String,
    pub first_sequence: u64,
    pub last_sequence: u64,
    pub first_signed_at: i64,
    pub last_signed_at: i64,
}

/// What the ledger holds for one resource.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceLedger {
    /// An epoch at or below this is refused: set by readmission above everything it read.
    #[serde(default)]
    pub epoch_floor: i64,
    /// A policy change from a serial at or below this is refused; none when absent.
    #[serde(default)]
    pub serial_floor: Option<i64>,
    /// Promises of takeover certificates, by epoch.
    #[serde(default)]
    pub epochs: BTreeMap<i64, Promise>,
    /// Promises of policy-change certificates, by `from_serial`.
    #[serde(default)]
    pub serials: BTreeMap<i64, Promise>,
}

impl ResourceLedger {
    fn promises(&self, kind: PromiseKind) -> &BTreeMap<i64, Promise> {
        match kind {
            PromiseKind::Epoch => &self.epochs,
            PromiseKind::Serial => &self.serials,
        }
    }

    fn promises_mut(&mut self, kind: PromiseKind) -> &mut BTreeMap<i64, Promise> {
        match kind {
            PromiseKind::Epoch => &mut self.epochs,
            PromiseKind::Serial => &mut self.serials,
        }
    }

    /// The highest number promised or floored, for readmission's floor.
    #[must_use]
    pub fn highest(&self, kind: PromiseKind) -> Option<i64> {
        let floor = match kind {
            PromiseKind::Epoch => Some(self.epoch_floor).filter(|f| *f > 0),
            PromiseKind::Serial => self.serial_floor,
        };
        self.promises(kind).keys().next_back().copied().max(floor)
    }
}

/// One input a readmission read, and its digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputRead {
    pub input: String,
    pub source: String,
    pub sha256: String,
    pub collected_at: Option<i64>,
}

/// The floors a readmission set for one resource.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FloorSet {
    pub epoch_floor: i64,
    pub serial_floor: Option<i64>,
}

/// The record of one readmission: what it read, and what it set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Admission {
    pub at: i64,
    pub operation_id: String,
    pub by_uid: Option<u32>,
    pub unadmitted_since: i64,
    pub unadmitted_reason: String,
    pub inputs_read: Vec<InputRead>,
    pub votes_counted: usize,
    /// Votes read that did not count, with why (a signature that does not verify cannot be in any
    /// certificate a node accepts).
    pub votes_ignored: Vec<String>,
    pub floors_set: BTreeMap<String, FloorSet>,
    pub sequence_before: u64,
    pub sequence_set: u64,
}

/// The ledger file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ledger {
    pub form: String,
    pub key_id: String,
    pub public_key: String,
    pub machine_id: String,
    pub creation_nonce: String,
    pub created_at: i64,
    /// The last sequence number issued.
    pub sequence: u64,
    pub admitted: bool,
    /// The sequence at the last admission: every vote of this key numbered above it was signed by
    /// this ledger since, and must be in it.
    pub admitted_at_sequence: u64,
    pub unadmitted_since: Option<i64>,
    pub unadmitted_reason: Option<String>,
    pub resources: BTreeMap<String, ResourceLedger>,
    pub admissions: Vec<Admission>,
    /// SHA-256 of the canonical JSON of the ledger with this field empty.
    pub checksum: String,
}

impl Ledger {
    fn computed_checksum(&self) -> Result<String, LedgerRefusal> {
        let mut unsummed = self.clone();
        unsummed.checksum = String::new();
        let value = serde_json::to_value(&unsummed)
            .map_err(|e| refusal("ledger_unreadable", e.to_string()))?;
        Ok(quorum::sha256_hex(
            &quorum::canonical_json(&value)
                .map_err(|e| refusal("ledger_unreadable", e.to_string()))?,
        ))
    }

    /// Parses a ledger file's bytes and checks its form and checksum, not whom it belongs to.
    ///
    /// # Errors
    /// `ledger_unreadable`.
    pub fn parse(bytes: &[u8]) -> Result<Ledger, LedgerRefusal> {
        let ledger: Ledger = serde_json::from_slice(bytes).map_err(|e| {
            refusal(
                "ledger_unreadable",
                format!("the ledger does not parse: {e}"),
            )
        })?;
        if ledger.form != LEDGER_FORM {
            return Err(refusal(
                "ledger_unreadable",
                format!("the ledger's form is not {LEDGER_FORM}"),
            ));
        }
        if ledger.checksum != ledger.computed_checksum()? {
            return Err(refusal(
                "ledger_unreadable",
                "the ledger's checksum does not match its content",
            ));
        }
        Ok(ledger)
    }

    /// The promise, if any, for one (kind, resource, number).
    #[must_use]
    pub fn promise(&self, kind: PromiseKind, resource: &str, number: i64) -> Option<&Promise> {
        self.resources.get(resource)?.promises(kind).get(&number)
    }
}

/// What the signer checks a payload's life against.
#[derive(Clone, Copy, Debug)]
pub struct SigningRules {
    /// The longest life (`expires_at - issued_at`) a vote may give a certificate. Readmission waits
    /// this long, and the clock skew, after a ledger is marked unadmitted, so that a signature it
    /// made and forgot, still in a proposer's hands, has expired before the key signs again.
    pub max_certificate_life: i64,
    /// How long a signer waits for the ledger's lock.
    pub lock_wait: Duration,
}

#[cfg(test)]
fn point(name: &str) {
    points::reach(name);
}

#[cfg(not(test))]
fn point(_name: &str) {}

/// Test-only: the moment a vote leaves the signer, made observable to a test process that crashes
/// its signer (`PODMESH_VOTE_TEST_RELEASE` names the file the vote is written to).
#[cfg(test)]
fn release(vote: &Value) {
    if let Ok(path) = std::env::var("PODMESH_VOTE_TEST_RELEASE") {
        let _ = fs::write(path, vote.to_string());
    }
}

#[cfg(not(test))]
fn release(_vote: &Value) {}

/// The live hypervisor generation witness a signer watches: the file the hypervisor holds outside
/// the guest's snapshot, and the value `guard_generation` accepted from it.
struct WatchedGeneration {
    path: PathBuf,
    observed: String,
    identity: WitnessIdentity,
}

/// What a witness was, when it was accepted: the filesystem it is on and the file it is. Checked
/// again before every read, because a path is not a file: a symlink flipped, a mount laid over it or
/// a file replaced since would otherwise be read on the strength of a check made once at startup.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct WitnessIdentity {
    filesystem: rustix::fs::FsWord,
    device: u64,
    inode: u64,
}

/// What a signer knows about its hypervisor generation witness.
enum GenerationWatch {
    /// Nothing has been said yet. A signer left in this state refuses to sign and refuses to
    /// readmit: an unanswered question is not an answer.
    Unset,
    /// The witness the host provides, and the value `guard_generation` accepted from it.
    Watched(WatchedGeneration),
    /// The operator's recorded decision to sign without a witness, and the reason given. The
    /// replica then has no defence against a snapshot rollback, and says so in its status.
    Waived(String),
}

/// The replica's signer: its key, its ledger, its host, the policy it votes under, and the live
/// generation witness it watches, if its host provides one.
pub struct Signer {
    dir: PathBuf,
    key_id: String,
    key: SigningKey,
    public_key: String,
    host: HostIdentity,
    policy: Quorum,
    rules: SigningRules,
    generation: GenerationWatch,
}

/// The lock on a ledger, released when dropped.
pub(crate) struct LedgerLock(File);

impl Drop for LedgerLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

fn private_directory(dir: &Path) -> Result<(), LedgerRefusal> {
    let metadata = fs::symlink_metadata(dir)
        .map_err(|e| refusal("vote_dir_unsafe", format!("{}: {e}", dir.display())))?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(refusal(
            "vote_dir_unsafe",
            "the vote directory must be a directory of this user, closed to group and others",
        ));
    }
    Ok(())
}

fn private_file(path: &Path, what: &'static str) -> Result<(), LedgerRefusal> {
    let metadata = fs::symlink_metadata(path).map_err(|e| {
        refusal(
            if e.kind() == std::io::ErrorKind::NotFound && what == "ledger_unreadable" {
                "ledger_missing"
            } else {
                what
            },
            format!("{}: {e}", path.display()),
        )
    })?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(refusal(
            what,
            format!(
                "{} must be a regular, singly linked file of this user, closed to group and others",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn read_bounded(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(std::io::Error::other("file exceeds its bound"));
    }
    Ok(bytes)
}

fn random_hex() -> Result<String, LedgerRefusal> {
    let mut bytes = [0_u8; 32];
    getrandom::getrandom(&mut bytes)
        .map_err(|_| refusal("storage", "OS randomness unavailable"))?;
    Ok(quorum::hex(&bytes))
}

impl Signer {
    /// Opens the signer of `key_id` in `dir`: reads the key file `<key_id>.key` (the 32-byte seed as
    /// 64 lowercase hex characters, a private file of this user), and checks the policy names this
    /// key with this public half. Does not read the ledger.
    ///
    /// # Errors
    /// `vote_dir_unsafe`, `key_unreadable`, `key_not_in_policy`.
    pub fn open(
        dir: &Path,
        key_id: &str,
        host: HostIdentity,
        policy: Quorum,
        rules: SigningRules,
    ) -> Result<Signer, LedgerRefusal> {
        quorum::check_key_id(key_id).map_err(|e| refusal("key_unreadable", e.to_string()))?;
        private_directory(dir)?;
        let key_path = dir.join(format!("{key_id}.key"));
        private_file(&key_path, "key_unreadable")?;
        let text =
            read_bounded(&key_path, 129).map_err(|e| refusal("key_unreadable", e.to_string()))?;
        let text = std::str::from_utf8(&text)
            .map_err(|_| refusal("key_unreadable", "the key file is not text"))?;
        let seed: [u8; 32] = quorum::hex_decode(text.trim_end_matches('\n'), "the key file", 32)
            .ok()
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| {
                refusal(
                    "key_unreadable",
                    "the key file holds a 32-byte seed as 64 lowercase hex characters",
                )
            })?;
        let key = SigningKey::from_bytes(&seed);
        let public_key = quorum::hex(&key.verifying_key().to_bytes());
        match policy.key(key_id) {
            Some(k) if k.public_key == public_key => {}
            Some(_) => {
                return Err(refusal(
                    "key_not_in_policy",
                    format!("the policy names {key_id} with another public key"),
                ))
            }
            None => {
                return Err(refusal(
                    "key_not_in_policy",
                    format!("the policy names no key {key_id}"),
                ))
            }
        }
        Ok(Signer {
            dir: dir.to_path_buf(),
            key_id: key_id.to_string(),
            key,
            public_key,
            host,
            policy,
            rules,
            generation: GenerationWatch::Unset,
        })
    }

    #[must_use]
    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    #[must_use]
    pub fn verifying_key(&self) -> VerifyingKey {
        self.key.verifying_key()
    }

    #[must_use]
    pub fn policy(&self) -> &Quorum {
        &self.policy
    }

    #[must_use]
    pub fn rules(&self) -> SigningRules {
        self.rules
    }

    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    #[must_use]
    pub fn ledger_path(&self) -> PathBuf {
        self.dir.join(format!("{}.ledger", self.key_id))
    }

    fn temp_path(&self) -> PathBuf {
        self.dir.join(format!(".{}.ledger.tmp", self.key_id))
    }

    /// What the lock is taken on: the vote directory itself, opened without following a symlink.
    /// A lock file can be removed or replaced under a live signer, and the next signer then locks a
    /// new inode (review of V3-4, finding 3); the directory holds the key and the ledger, and cannot
    /// be removed while they are in it. Returns the open directory and the path it must still be.
    fn lock_target(&self) -> Result<(File, PathBuf), LedgerRefusal> {
        use rustix::fs::{open, Mode, OFlags};
        let fd = open(
            &self.dir,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| refusal("storage", format!("the vote directory, to lock: {e}")))?;
        Ok((File::from(fd), self.dir.clone()))
    }

    /// Takes the exclusive lock of this signer's vote directory, waiting at most the rules' lock
    /// wait. After locking, the directory's path must still name the inode locked; if it was renamed
    /// or replaced meanwhile, the lock is dropped and taken again on what the path names now.
    pub(crate) fn lock(&self) -> Result<LedgerLock, LedgerRefusal> {
        use std::os::unix::fs::MetadataExt as _;
        let deadline = Instant::now() + self.rules.lock_wait;
        loop {
            let (file, path) = self.lock_target()?;
            loop {
                match file.try_lock_exclusive() {
                    Ok(()) => break,
                    Err(_) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => {
                        return Err(refusal(
                            "ledger_busy",
                            "another signer holds this ledger's lock",
                        ))
                    }
                }
            }
            let locked = file
                .metadata()
                .map_err(|e| refusal("storage", e.to_string()))?;
            let named =
                fs::symlink_metadata(&path).map_err(|e| refusal("storage", e.to_string()))?;
            if (locked.dev(), locked.ino()) == (named.dev(), named.ino()) {
                return Ok(LedgerLock(file));
            }
            if Instant::now() >= deadline {
                return Err(refusal(
                    "ledger_busy",
                    "the locked vote directory was replaced under its path while waiting",
                ));
            }
        }
    }

    /// Reads the ledger and checks whom it belongs to, admitted or not.
    ///
    /// # Errors
    /// `ledger_missing`, `ledger_unreadable`, `ledger_foreign_key`, `ledger_foreign_host`.
    pub fn load(&self) -> Result<Ledger, LedgerRefusal> {
        let path = self.ledger_path();
        private_file(&path, "ledger_unreadable")?;
        let bytes = read_bounded(&path, LEDGER_MAX_BYTES)
            .map_err(|e| refusal("ledger_unreadable", e.to_string()))?;
        let ledger = Ledger::parse(&bytes)?;
        if ledger.key_id != self.key_id || ledger.public_key != self.public_key {
            return Err(refusal(
                "ledger_foreign_key",
                format!(
                    "the ledger was made for key {} ({}), not this one",
                    ledger.key_id, ledger.public_key
                ),
            ));
        }
        if ledger.machine_id != self.host.machine_id() {
            return Err(refusal(
                "ledger_foreign_host",
                format!(
                    "the ledger was made on host {}, this host is {}: a copied or cloned ledger signs nothing",
                    ledger.machine_id,
                    self.host.machine_id()
                ),
            ));
        }
        Ok(ledger)
    }

    /// Writes the ledger durably: a temporary file, fsync, rename over the ledger, fsync of the
    /// directory. Nothing is signed before this returns.
    pub(crate) fn store(&self, ledger: &mut Ledger) -> Result<(), LedgerRefusal> {
        ledger.checksum = ledger.computed_checksum()?;
        let bytes =
            serde_json::to_vec_pretty(&*ledger).map_err(|e| refusal("storage", e.to_string()))?;
        let temp = self.temp_path();
        let io = |e: std::io::Error| {
            refusal(
                "storage",
                format!("the ledger could not be written durably: {e}"),
            )
        };
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&temp)
            .map_err(io)?;
        file.write_all(&bytes).map_err(io)?;
        point("after_write");
        file.sync_all().map_err(io)?;
        drop(file);
        fs::rename(&temp, self.ledger_path()).map_err(io)?;
        point("after_rename");
        File::open(&self.dir)
            .and_then(|d| d.sync_all())
            .map_err(io)?;
        point("after_commit");
        Ok(())
    }

    /// Creates this key's ledger, unadmitted: it signs nothing until the operator's readmission.
    ///
    /// # Errors
    /// `ledger_exists`, and the storage refusals.
    pub fn init(&self, now: i64) -> Result<Ledger, LedgerRefusal> {
        let _lock = self.lock()?;
        match fs::symlink_metadata(self.ledger_path()) {
            Ok(_) => {
                return Err(refusal(
                    "ledger_exists",
                    "this key already has a ledger; a new ledger is made only for a new key",
                ))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(refusal("storage", e.to_string())),
        }
        let mut ledger = Ledger {
            form: LEDGER_FORM.into(),
            key_id: self.key_id.clone(),
            public_key: self.public_key.clone(),
            machine_id: self.host.machine_id().into(),
            creation_nonce: random_hex()?,
            created_at: now,
            sequence: 0,
            admitted: false,
            admitted_at_sequence: 0,
            unadmitted_since: Some(now),
            unadmitted_reason: Some(
                "created: a new ledger is admitted by the operator's readmission".into(),
            ),
            resources: BTreeMap::new(),
            admissions: Vec::new(),
            checksum: String::new(),
        };
        self.store(&mut ledger)?;
        Ok(ledger)
    }

    /// Marks the ledger unadmitted, as the restore procedure does after any restore of the host or
    /// revert of its snapshot: it signs nothing until the operator's readmission. Marking a ledger
    /// already unadmitted keeps its first mark, so the wait is not restarted.
    ///
    /// # Errors
    /// The ledger's refusals.
    pub fn mark_unadmitted(&self, reason: &str, now: i64) -> Result<Ledger, LedgerRefusal> {
        let _lock = self.lock()?;
        let mut ledger = self.load()?;
        if ledger.admitted {
            ledger.admitted = false;
            ledger.unadmitted_since = Some(now);
            ledger.unadmitted_reason = Some(reason.to_string());
            self.store(&mut ledger)?;
        }
        Ok(ledger)
    }

    /// Before a voting resident starts exchanging with peers, bind its ledger to this kernel boot.
    /// A file-based VM restore boots with a new kernel boot ID while restoring the old ledger and
    /// marker together. In that case the ledger is durably unadmitted before the marker advances.
    /// A crash between the two writes only causes another safe mark on the next start.
    pub fn guard_boot(&self, boot_id: &str, now: i64) -> Result<(), LedgerRefusal> {
        if boot_id.len() != 36
            || !boot_id.bytes().enumerate().all(|(i, b)| {
                if [8, 13, 18, 23].contains(&i) {
                    b == b'-'
                } else {
                    b.is_ascii_hexdigit() && !b.is_ascii_uppercase()
                }
            })
        {
            return Err(refusal(
                "boot_identity_unreadable",
                "the kernel boot ID is not a lowercase UUID",
            ));
        }
        self.guard_witness(
            "boot-id",
            boot_id,
            "boot_identity_unreadable",
            "kernel boot changed or boot witness missing: verify host restore before readmission",
            now,
        )
    }

    /// A hypervisor generation witness is outside the guest's snapshot. It catches a rollback
    /// that resumes kernel memory and therefore retains the old boot ID.
    ///
    /// This guard releases the ledger's lock again: a signer that will go on to sign or readmit
    /// must also be given the file the value was read from (`watch_generation`), so that the same
    /// witness is read once more, under the lock, before anything is released or admitted.
    pub fn guard_generation(&self, generation_id: &str, now: i64) -> Result<(), LedgerRefusal> {
        if generation_id.len() != 32
            || !generation_id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(refusal(
                "generation_identity_unreadable",
                "the hypervisor generation ID is not 16 bytes of lowercase hex",
            ));
        }
        self.guard_witness("generation-id", generation_id, "generation_identity_unreadable", "hypervisor generation changed or generation witness missing: verify VM restore before readmission", now)
    }

    fn guard_witness(
        &self,
        suffix: &str,
        observed: &str,
        error_code: &'static str,
        reason: &str,
        now: i64,
    ) -> Result<(), LedgerRefusal> {
        let _lock = self.lock()?;
        let marker = self.dir.join(format!("{}.{}", self.key_id, suffix));
        let previous = match fs::symlink_metadata(&marker) {
            Ok(_) => {
                private_file(&marker, error_code)?;
                let bytes =
                    read_bounded(&marker, 64).map_err(|e| refusal(error_code, e.to_string()))?;
                Some(String::from_utf8(bytes).map_err(|e| refusal(error_code, e.to_string()))?)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(refusal(error_code, e.to_string())),
        };
        if previous.as_deref() == Some(observed) {
            return Ok(());
        }
        match fs::symlink_metadata(self.ledger_path()) {
            Ok(_) => {
                let mut ledger = self.load()?;
                if ledger.admitted {
                    ledger.admitted = false;
                    ledger.unadmitted_since = Some(now);
                    ledger.unadmitted_reason = Some(reason.into());
                    self.store(&mut ledger)?;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(refusal("storage", e.to_string())),
        }
        let temp = self.dir.join(format!(".{}.{}.tmp", self.key_id, suffix));
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&temp)
            .map_err(|e| refusal("storage", e.to_string()))?;
        file.write_all(observed.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|e| refusal("storage", e.to_string()))?;
        drop(file);
        fs::rename(&temp, &marker).map_err(|e| refusal("storage", e.to_string()))?;
        File::open(&self.dir)
            .and_then(|d| d.sync_all())
            .map_err(|e| refusal("storage", e.to_string()))?;
        Ok(())
    }

    /// Watches the live generation witness at `path`, whose value `guard_generation` has just
    /// accepted, for the rest of this signer's life: `sign` reads the file again under the signing
    /// lock, before it mutates the ledger and again immediately before a vote is sealed, and
    /// readmission before it admits.
    ///
    /// The witness is only worth reading again if it lives outside the guest's snapshot, so this
    /// checks the filesystem it is on before accepting it. A rollback that restores the guest's
    /// memory restores its page cache too; a witness on an ordinary filesystem would then answer
    /// with the value it held before the rollback, and the second read would prove nothing.
    ///
    /// # Errors
    /// `generation_witness_untrusted`: the witness is not on a filesystem the hypervisor answers
    /// for. Use `waive_generation` instead if the operator decides to sign without the defence.
    pub fn watch_generation(&mut self, path: PathBuf, observed: &str) -> Result<(), LedgerRefusal> {
        let identity = trust_generation_witness(&path)?;
        self.generation = GenerationWatch::Watched(WatchedGeneration {
            path,
            observed: observed.to_string(),
            identity,
        });
        Ok(())
    }

    /// Adopts the witness at `path` in one ordered act: judge the path, read it, guard the ledger
    /// against the marker, and watch it. Nothing durable is marked before the path is judged, and
    /// there is one order rather than one per caller.
    ///
    /// # Errors
    /// `generation_witness_untrusted`, `generation_identity_unreadable`, and the guard's.
    pub fn adopt_generation_witness(
        &mut self,
        path: PathBuf,
        now: i64,
    ) -> Result<String, LedgerRefusal> {
        let identity = trust_generation_witness(&path)?;
        let observed = crate::votes::generation_id_from(&path)?;
        self.guard_generation(&observed, now)?;
        self.generation = GenerationWatch::Watched(WatchedGeneration {
            path,
            observed: observed.clone(),
            identity,
        });
        Ok(observed)
    }

    /// Watches a witness without checking the filesystem it is on. The laboratory's suites use it
    /// against ordinary files; a replica that may sign uses `watch_generation`, which refuses one.
    #[cfg(test)]
    pub(crate) fn watch_generation_unchecked(&mut self, path: PathBuf, observed: &str) {
        let identity = current_witness_identity(&path).unwrap_or(WitnessIdentity {
            filesystem: 0,
            device: 0,
            inode: 0,
        });
        self.generation = GenerationWatch::Watched(WatchedGeneration {
            path,
            observed: observed.to_string(),
            identity,
        });
    }

    /// Records the operator's decision to sign with no generation witness, and why. The signer then
    /// signs, and every rollback this check would have caught passes unseen: the decision is the
    /// operator's, it is named, and `status` shows it for as long as it holds.
    pub fn waive_generation(&mut self, reason: impl Into<String>) -> Result<(), LedgerRefusal> {
        let reason = reason.into();
        if reason.trim().is_empty() {
            return Err(refusal(
                GENERATION_WITNESS_ABSENT,
                "a waiver with no reason is not a waiver: name why this guest is beyond the reach of snapshots",
            ));
        }
        self.generation = GenerationWatch::Waived(reason);
        Ok(())
    }

    /// The reason a witness was waived, if one was.
    pub fn generation_waiver(&self) -> Option<&str> {
        match &self.generation {
            GenerationWatch::Waived(reason) => Some(reason.as_str()),
            _ => None,
        }
    }

    /// Reads the watched generation witness again. The hypervisor holds it outside the guest's
    /// snapshot: a guest that went back to another generation since the guard, taking this
    /// process's memory and its files with it, reads another value here.
    ///
    /// # Errors
    /// `generation_changed`: the witness no longer holds the value guarded; the caller releases
    /// nothing and the ledger is quarantined. `generation_identity_unreadable`: the witness cannot
    /// be read now, so nothing shows the generation did not change, and the caller fails closed.
    /// `generation_witness_absent`: nothing has said whether this signer has a witness at all.
    pub(crate) fn check_generation(&self) -> Result<(), LedgerRefusal> {
        let watched = match &self.generation {
            GenerationWatch::Watched(watched) => watched,
            GenerationWatch::Waived(_) => return Ok(()),
            GenerationWatch::Unset => {
                return Err(refusal(
                    GENERATION_WITNESS_ABSENT,
                    "no hypervisor generation witness was watched and none was waived: this signer cannot tell whether its guest went back to an earlier generation, and it signs nothing until one or the other is recorded",
                ))
            }
        };
        // The path is judged again, not only when it was adopted: a symlink flipped, a mount laid
        // over it or a file replaced since would otherwise be read on a check made once at startup.
        // A witness that cannot be stat'd at all is the unreadable case, not the untrusted one:
        // nothing shows the generation did not change, and the caller fails closed either way, but
        // the two say different things to whoever reads the refusal.
        let now_identity = current_witness_identity(&watched.path)
            .map_err(|e| refusal("generation_identity_unreadable", e.detail))?;
        if now_identity != watched.identity {
            return Err(refusal(
                GENERATION_WITNESS_UNTRUSTED,
                format!(
                    "the witness at {} is no longer the file that was accepted: it was replaced, remounted or redirected since, and what it says now stands for nothing",
                    watched.path.display()
                ),
            ));
        }
        let observed = crate::votes::generation_id_from(&watched.path)?;
        if observed == watched.observed {
            return Ok(());
        }
        Err(refusal(
            GENERATION_CHANGED,
            format!(
                "the hypervisor's generation is {observed}, this signer was guarded on {}: the guest moved to another generation (a snapshot resume or a restore) since, and everything inside it may have gone back with it",
                watched.observed
            ),
        ))
    }

    /// Reads the watched witness and, when it says the guest moved, marks the ledger unadmitted
    /// durably before refusing. A witness that is absent, untrusted or unreadable refuses without
    /// quarantining: those say the check could not be made, not that a rollback happened.
    fn guard_generation_or_quarantine(
        &self,
        ledger: &mut Ledger,
        now: i64,
    ) -> Result<(), LedgerRefusal> {
        let Err(alarm) = self.check_generation() else {
            return Ok(());
        };
        if alarm.code == GENERATION_CHANGED {
            ledger.admitted = false;
            ledger.unadmitted_since = Some(now);
            ledger.unadmitted_reason = Some(format!(
                "generation witness ({}): {}; it signs nothing until the operator readmits it",
                alarm.code, alarm.detail
            ));
            self.store(ledger)?;
        }
        Err(alarm)
    }

    /// The tripwire: a vote of this key, among those shown, that the ledger cannot account for.
    fn tripwire(&self, ledger: &Ledger, seen: &[Value]) -> Option<LedgerRefusal> {
        let own: BTreeMap<String, VerifyingKey> =
            [(self.key_id.clone(), self.key.verifying_key())].into();
        for v in seen {
            if v["voter"].as_str() != Some(self.key_id.as_str()) {
                continue;
            }
            // Only a vote this key really signed counts: a forged one cannot fire the tripwire.
            let Ok(verified) = vote::verify_origin(v, &own) else {
                continue;
            };
            if verified.ledger_sequence > ledger.sequence {
                return Some(refusal(
                    "ledger_behind_own_votes",
                    format!(
                        "a vote of key {} numbered {} exists, and this ledger is at {}: the ledger went back in time (a restore or a snapshot revert); it signs nothing until the operator readmits it",
                        self.key_id, verified.ledger_sequence, ledger.sequence
                    ),
                ));
            }
            let d = &verified.decision;
            if verified.ledger_sequence > ledger.admitted_at_sequence
                && ledger
                    .promise(d.kind, &d.resource, d.number)
                    .map(|p| p.decision_digest.as_str())
                    != Some(d.decision_digest.as_str())
            {
                return Some(refusal(
                    "own_vote_unknown_to_ledger",
                    format!(
                        "a vote of key {} numbered {} for {} {:?} {} is not in this ledger, which admitted it at {}: the ledger lost a promise it made",
                        self.key_id, verified.ledger_sequence, d.resource, d.kind, d.number, ledger.admitted_at_sequence
                    ),
                ));
            }
        }
        None
    }

    /// Signs a vote for `payload` (a certificate payload of V3-2, without `signatures`), at most once
    /// per (resource, epoch) and once per (resource, from_serial), after writing the promise durably.
    /// `seen` are the votes this replica can see (its peers' facts and its own): the tripwire reads
    /// them. `now` is this replica's clock, in seconds.
    ///
    /// # Errors
    /// The ledger's refusals; the tripwire's and `generation_changed` (the ledger is then marked
    /// unadmitted); `generation_identity_unreadable`; `payload_invalid`,
    /// `policy_mismatch`, `certificate_life`, `serial_not_current`; and the promise rule's:
    /// `epoch_at_or_below_floor`, `epoch_superseded`, `epoch_already_promised`,
    /// `serial_at_or_below_floor`, `serial_superseded`, `serial_already_promised`.
    pub fn sign(&self, payload: &Value, seen: &[Value], now: i64) -> Result<Value, LedgerRefusal> {
        let _lock = self.lock()?;
        let mut ledger = self.load()?;
        if !ledger.admitted {
            return Err(refusal(
                "ledger_unadmitted",
                format!(
                    "the ledger is unadmitted since {} ({}); the operator's readmission admits it",
                    ledger.unadmitted_since.unwrap_or_default(),
                    ledger
                        .unadmitted_reason
                        .as_deref()
                        .unwrap_or("no reason recorded")
                ),
            ));
        }
        if let Some(alarm) = self.tripwire(&ledger, seen) {
            ledger.admitted = false;
            ledger.unadmitted_since = Some(now);
            ledger.unadmitted_reason = Some(format!("tripwire ({}): {}", alarm.code, alarm.detail));
            self.store(&mut ledger)?;
            return Err(alarm);
        }
        let decision =
            Decision::of_payload(payload).map_err(|e| refusal("payload_invalid", e.detail))?;
        if payload["authority_id"].as_str() != Some(self.policy.authority_id.as_str())
            || payload["policy_digest"].as_str() != Some(self.policy.digest().as_str())
        {
            return Err(refusal(
                "policy_mismatch",
                "the payload names another authority set than the one this replica votes under",
            ));
        }
        if decision.issued_at > now + CLOCK_SKEW_SECONDS
            || decision.expires_at <= now
            || decision.expires_at <= decision.issued_at
            || decision.expires_at - decision.issued_at > self.rules.max_certificate_life
        {
            return Err(refusal(
                "certificate_life",
                format!(
                    "a vote is signed only for a payload live on this clock, issued no more than {CLOCK_SKEW_SECONDS} s ahead, living at most {} s",
                    self.rules.max_certificate_life
                ),
            ));
        }
        if decision.kind == PromiseKind::Serial
            && u64::try_from(decision.number).ok() != Some(self.policy.serial)
        {
            return Err(refusal(
                "serial_not_current",
                format!("a replica votes only for a change away from its own authority set, at serial {}", self.policy.serial),
            ));
        }
        // The witness is read twice. This first read is the last moment at which the ledger is
        // still exactly what was loaded: `guard_generation` ran before this lock was taken, and a
        // snapshot resume since would have taken this process, the ledger and the marker back with
        // it, so only the hypervisor's own witness still says so. Refusing here leaves no promise
        // behind for a vote that was never released -- a promise the ledger would otherwise hold
        // against a different holder at the same number, for nothing.
        self.guard_generation_or_quarantine(&mut ledger, now)?;
        point("after_check");
        let resource = ledger
            .resources
            .entry(decision.resource.clone())
            .or_default();
        let (floor, names) = match decision.kind {
            PromiseKind::Epoch => (
                Some(resource.epoch_floor),
                (
                    "epoch_at_or_below_floor",
                    "epoch_superseded",
                    "epoch_already_promised",
                ),
            ),
            PromiseKind::Serial => (
                resource.serial_floor,
                (
                    "serial_at_or_below_floor",
                    "serial_superseded",
                    "serial_already_promised",
                ),
            ),
        };
        if floor.is_some_and(|f| decision.number <= f) {
            return Err(refusal(
                names.0,
                format!(
                    "{} is at or below this ledger's floor {} for {}, set by readmission",
                    decision.number,
                    floor.unwrap_or_default(),
                    decision.resource
                ),
            ));
        }
        let promises = resource.promises_mut(decision.kind);
        if let Some((&later, _)) = promises.range(decision.number + 1..).next() {
            return Err(refusal(
                names.1,
                format!(
                    "this key already promised {later} for {}, above {}",
                    decision.resource, decision.number
                ),
            ));
        }
        let sequence = ledger.sequence + 1;
        match promises.get_mut(&decision.number) {
            Some(p) if p.decision_digest != decision.decision_digest => {
                return Err(refusal(
                    names.2,
                    format!(
                        "this key already promised {} for {} to {}; it promises once",
                        decision.number, decision.resource, p.holder
                    ),
                ));
            }
            Some(p) => {
                p.last_sequence = sequence;
                p.last_signed_at = now;
                p.payload_digest.clone_from(&decision.payload_digest);
            }
            None => {
                promises.insert(
                    decision.number,
                    Promise {
                        holder: decision.holder.clone(),
                        decision_digest: decision.decision_digest.clone(),
                        payload_digest: decision.payload_digest.clone(),
                        first_sequence: sequence,
                        last_sequence: sequence,
                        first_signed_at: now,
                        last_signed_at: now,
                    },
                );
            }
        }
        ledger.sequence = sequence;
        self.store(&mut ledger)?;
        // And again, as the last act before a signature of this key exists, because the ledger was
        // written since the first read and a resume between the two would have taken that write
        // back with it. A changed witness marks the ledger unadmitted, durably, and nothing is
        // released; a witness that cannot be read proves nothing either, so nothing is released
        // then either. The promise stays: an over-promise is the direction a crash here already
        // leaves, and it is the one that never forgets a vote that did go out.
        self.guard_generation_or_quarantine(&mut ledger, now)?;
        let vote = vote::seal(
            &self.key,
            &self.key_id,
            payload,
            &LedgerStamp {
                nonce: &ledger.creation_nonce,
                sequence,
            },
        )
        .map_err(|e| refusal("payload_invalid", e.detail))?;
        release(&vote);
        Ok(vote)
    }
}

/// Test-only crash and pause points, reached through the environment of a test process. Compiled out
/// of every build that is not a test.
#[cfg(test)]
pub(crate) mod points {
    pub const CRASH: &str = "PODMESH_VOTE_TEST_CRASH";
    pub const PAUSE: &str = "PODMESH_VOTE_TEST_PAUSE";
    pub const PAUSE_MS: &str = "PODMESH_VOTE_TEST_PAUSE_MS";
    /// A file written when the pause is reached, before sleeping.
    pub const PAUSE_MARK: &str = "PODMESH_VOTE_TEST_PAUSE_MARK";

    pub fn reach(name: &str) {
        if std::env::var(CRASH).as_deref() == Ok(name) {
            std::process::abort();
        }
        if std::env::var(PAUSE).as_deref() == Ok(name) {
            if let Ok(mark) = std::env::var(PAUSE_MARK) {
                let _ = std::fs::write(mark, name);
            }
            let ms = std::env::var(PAUSE_MS)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(300);
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
    }
}
