//! Offline, explicitly enrolled observation journal. No runtime or network authority.
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const MAX_BODY: usize = 65_536;
const MAX_EVENTS: usize = 4096;
const MAX_BYTES: usize = 16 * 1024 * 1024;
fn quota(db: &Connection, additional: usize, request: bool) -> Result<()> {
    let bytes: usize = db.query_row("SELECT (SELECT coalesce(sum(length(body)),0) FROM events) + (SELECT coalesce(sum(length(body)),0) FROM requests)", [], |r| r.get(0))?;
    let requests: usize = db.query_row("SELECT count(*) FROM requests", [], |r| r.get(0))?;
    if bytes.saturating_add(additional) > MAX_BYTES || (request && requests >= MAX_EVENTS) {
        return Err("lab retained-history quota exhausted; no automatic eviction".into());
    }
    Ok(())
}

// Typed fields avoid Value's duplicate-key loss; unknown/duplicate fields are refused.
// This deliberately narrow profile admits observations, not exclusive claims.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub format: String,
    pub mesh_uuid: String,
    pub origin_replica_uuid: String,
    pub producer_epoch_uuid: String,
    pub sequence: u64,
    pub previous_event_id: Option<String>,
    pub dependencies: Vec<String>,
    pub event_type: String,
    pub resource_key: String,
    pub observed_at: String,
    pub evidence_digest: String,
    pub reported_state: String,
}
fn uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit() && !b.is_ascii_uppercase()
            }
        })
}
fn hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn unique_fields(bytes: &[u8]) -> Result<()> {
    struct Keys;
    impl<'de> serde::de::Visitor<'de> for Keys {
        type Value = ();
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("an object with unique fields")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> std::result::Result<(), A::Error> {
            let mut seen = BTreeSet::new();
            while let Some(key) = map.next_key::<String>()? {
                if !seen.insert(key) {
                    return Err(serde::de::Error::custom("duplicate field"));
                }
                map.next_value::<serde::de::IgnoredAny>()?;
            }
            Ok(())
        }
    }
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    serde::de::Deserializer::deserialize_map(&mut decoder, Keys)?;
    decoder.end()?;
    Ok(())
}
fn decode(bytes: &[u8]) -> Result<Observation> {
    if bytes.len() > MAX_BODY {
        return Err("event too large".into());
    }
    unique_fields(bytes)?;
    let e: Observation = serde_json::from_slice(bytes)?;
    if e.format != "podmesh-registry-observation-lab/1"
        || e.event_type != "observation.reported"
        || !uuid(&e.mesh_uuid)
        || !uuid(&e.origin_replica_uuid)
        || !uuid(&e.producer_epoch_uuid)
        || e.sequence == 0
        || e.sequence > 9_007_199_254_740_991
        || e.dependencies.len() > 64
        || !e.dependencies.iter().all(|s| hash(s))
        || e.dependencies.iter().collect::<BTreeSet<_>>().len() != e.dependencies.len()
        || !hash(&e.evidence_digest)
        || e.resource_key.is_empty()
        || e.resource_key.len() > 512
        || e.observed_at.is_empty()
        || e.observed_at.len() > 64
        || !["running", "stopped", "unknown"].contains(&e.reported_state.as_str())
        || (e.sequence == 1) != e.previous_event_id.is_none()
        || e.previous_event_id.as_ref().is_some_and(|s| !hash(s))
    {
        return Err("invalid observation envelope".into());
    }
    Ok(e)
}

pub struct Store {
    db: Connection,
    mesh: String,
}
impl Store {
    pub fn open(path: &Path, mesh: &str) -> Result<Self> {
        if !uuid(mesh) {
            return Err("invalid mesh UUID".into());
        }
        let db = Connection::open(path)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
          CREATE TABLE IF NOT EXISTS identity(mesh TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS enrollments(epoch TEXT PRIMARY KEY, replica TEXT NOT NULL, prefix TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS events(id TEXT PRIMARY KEY, body BLOB NOT NULL, epoch TEXT NOT NULL, seq INTEGER NOT NULL);
          CREATE TABLE IF NOT EXISTS requests(epoch TEXT NOT NULL, request TEXT NOT NULL, body BLOB NOT NULL, event TEXT NOT NULL, PRIMARY KEY(epoch,request));
          CREATE TRIGGER IF NOT EXISTS events_no_update BEFORE UPDATE ON events BEGIN SELECT RAISE(ABORT,'immutable event'); END;
          CREATE TRIGGER IF NOT EXISTS events_no_delete BEFORE DELETE ON events BEGIN SELECT RAISE(ABORT,'immutable event'); END;")?;
        let tx = db.unchecked_transaction()?;
        let count: i64 = tx.query_row("SELECT count(*) FROM identity", [], |r| r.get(0))?;
        if count == 0 {
            tx.execute("INSERT INTO identity VALUES(?1)", [mesh])?;
        }
        let stored: String = tx.query_row("SELECT mesh FROM identity", [], |r| r.get(0))?;
        if stored != mesh || count > 1 {
            return Err("different mesh".into());
        }
        tx.commit()?;
        Ok(Self {
            db,
            mesh: mesh.into(),
        })
    }
    /// Local administrator fixture enrollment. No received event can enroll a writer.
    pub fn enroll(&self, epoch: &str, replica: &str, prefix: &str) -> Result<()> {
        if !uuid(epoch) || !uuid(replica) || prefix.is_empty() || prefix.len() > 512 {
            return Err("invalid enrollment".into());
        }
        let tx = rusqlite::Transaction::new_unchecked(
            &self.db,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let n: usize = tx.query_row("SELECT count(*) FROM enrollments", [], |r| r.get(0))?;
        if n >= 256 {
            return Err("lab enrollment quota exhausted".into());
        }
        tx.execute(
            "INSERT INTO enrollments VALUES(?1,?2,?3)",
            params![epoch, replica, prefix],
        )?;
        tx.commit()?;
        Ok(())
    }
    fn validate(&self, id: &str, body: &[u8]) -> Result<Observation> {
        if body.len() > MAX_BODY {
            return Err("event too large".into());
        }
        if digest(body) != id {
            return Err("hash mismatch".into());
        }
        let e = decode(body)?;
        if e.mesh_uuid != self.mesh || e.dependencies.iter().any(|d| d == id) {
            return Err("wrong mesh or self dependency".into());
        }
        let (replica, prefix): (String, String) = self.db.query_row(
            "SELECT replica,prefix FROM enrollments WHERE epoch=?1",
            [&e.producer_epoch_uuid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if replica != e.origin_replica_uuid || !e.resource_key.starts_with(&prefix) {
            return Err("origin or scope mismatch".into());
        }
        Ok(e)
    }
    /// A stored result is durable history, never verification of the reported state.
    pub fn ingest(&self, id: &str, body: &[u8]) -> Result<String> {
        let e = self.validate(id, body)?;
        let tx = self.db.unchecked_transaction()?;
        let existing: usize =
            tx.query_row("SELECT count(*) FROM events WHERE id=?1", [id], |r| {
                r.get(0)
            })?;
        if existing == 0 {
            let n: usize = tx.query_row("SELECT count(*) FROM events", [], |r| r.get(0))?;
            quota(&tx, body.len(), false)?;
            if n >= MAX_EVENTS {
                return Err("lab history quota exhausted; no automatic eviction".into());
            }
            tx.execute(
                "INSERT INTO events VALUES(?1,?2,?3,?4)",
                params![id, body, e.producer_epoch_uuid, e.sequence],
            )?;
        }
        tx.commit()?;
        Ok(self
            .states()?
            .get(id)
            .cloned()
            .ok_or("missing persisted event")?)
    }
    /// Idempotent exact-byte local submission, serialized with event persistence.
    /// Caller supplies the sequence; this first slice does not allocate writer epochs.
    pub fn submit(&self, request: &str, body: &[u8]) -> Result<String> {
        if !uuid(request) {
            return Err("invalid request UUID".into());
        }
        if body.len() > MAX_BODY {
            return Err("event too large".into());
        }
        let id = digest(body);
        let e = self.validate(&id, body)?;
        let tx = self.db.unchecked_transaction()?;
        use rusqlite::OptionalExtension;
        let old: Option<(Vec<u8>, String)> = tx
            .query_row(
                "SELECT body,event FROM requests WHERE epoch=?1 AND request=?2",
                params![e.producer_epoch_uuid, request],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((bytes, event)) = old {
            if bytes != body {
                return Err("request reused with changed bytes".into());
            }
            return Ok(event);
        }
        let n: usize = tx.query_row("SELECT count(*) FROM events", [], |r| r.get(0))?;
        quota(&tx, body.len().saturating_mul(2), true)?;
        if n >= MAX_EVENTS {
            return Err("lab history quota exhausted".into());
        }
        tx.execute(
            "INSERT OR IGNORE INTO events VALUES(?1,?2,?3,?4)",
            params![id, body, e.producer_epoch_uuid, e.sequence],
        )?;
        tx.execute(
            "INSERT INTO requests VALUES(?1,?2,?3,?4)",
            params![e.producer_epoch_uuid, request, body, id],
        )?;
        tx.commit()?;
        Ok(id)
    }
    pub fn export(&self) -> Result<Vec<(String, Vec<u8>)>> {
        let mut stmt = self.db.prepare("SELECT id,body FROM events ORDER BY id")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
    /// Rebuild from all immutable inputs. A fork invalidates the whole writer stream
    /// and transitively its consumers, irrespective of arrival order.
    pub fn states(&self) -> Result<BTreeMap<String, String>> {
        let mut events = BTreeMap::new();
        let mut slots: BTreeMap<(String, u64), usize> = BTreeMap::new();
        for (id, body) in self.export()? {
            let e = decode(&body)?;
            *slots
                .entry((e.producer_epoch_uuid.clone(), e.sequence))
                .or_default() += 1;
            events.insert(id, e);
        }
        let forks: BTreeSet<_> = slots
            .iter()
            .filter(|(_, n)| **n > 1)
            .map(|((epoch, _), _)| epoch.clone())
            .collect();
        let mut states: BTreeMap<String, String> = events
            .iter()
            .map(|(id, e)| {
                (
                    id.clone(),
                    if forks.contains(&e.producer_epoch_uuid) {
                        "quarantined"
                    } else {
                        "pending"
                    }
                    .into(),
                )
            })
            .collect();
        loop {
            let mut changed = false;
            for (id, e) in &events {
                if states[id] != "pending" {
                    continue;
                }
                let deps: Vec<_> = e
                    .dependencies
                    .iter()
                    .chain(e.previous_event_id.iter())
                    .collect();
                if deps.iter().any(|d| {
                    states
                        .get(*d)
                        .is_some_and(|s| s == "quarantined" || s == "invalid")
                }) {
                    states.insert(id.clone(), "quarantined".into());
                    changed = true;
                    continue;
                }
                if let Some(prev) = &e.previous_event_id {
                    if let Some(p) = events.get(prev) {
                        if p.producer_epoch_uuid != e.producer_epoch_uuid
                            || p.sequence + 1 != e.sequence
                        {
                            states.insert(id.clone(), "invalid".into());
                            changed = true;
                            continue;
                        }
                    }
                }
                if deps
                    .iter()
                    .all(|d| states.get(*d).is_some_and(|s| s == "admitted"))
                {
                    states.insert(id.clone(), "admitted".into());
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        Ok(states)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const M: &str = "00000000-0000-4000-8000-000000000001";
    const R: &str = "00000000-0000-4000-8000-000000000002";
    const E: &str = "00000000-0000-4000-8000-000000000003";
    const Q: &str = "00000000-0000-4000-8000-000000000004";
    fn store() -> Store {
        let s = Store::open(Path::new(":memory:"), M).unwrap();
        s.enroll(E, R, "host:a:").unwrap();
        s
    }
    fn event(seq: u64, prev: Option<String>) -> Vec<u8> {
        serde_json::to_vec(&Observation {
            format: "podmesh-registry-observation-lab/1".into(),
            mesh_uuid: M.into(),
            origin_replica_uuid: R.into(),
            producer_epoch_uuid: E.into(),
            sequence: seq,
            previous_event_id: prev,
            dependencies: vec![],
            event_type: "observation.reported".into(),
            resource_key: "host:a:workload".into(),
            observed_at: "2026-09-12T00:00:00Z".into(),
            evidence_digest: "a".repeat(64),
            reported_state: "unknown".into(),
        })
        .unwrap()
    }
    #[test]
    fn duplicate_and_request_reuse() {
        let s = store();
        let b = event(1, None);
        let id = s.submit(Q, &b).unwrap();
        assert_eq!(s.submit(Q, &b).unwrap(), id);
        assert_eq!(s.ingest(&id, &b).unwrap(), "admitted");
        assert_eq!(s.export().unwrap().len(), 1);
        let mut e = decode(&b).unwrap();
        e.reported_state = "running".into();
        assert!(s.submit(Q, &serde_json::to_vec(&e).unwrap()).is_err());
    }
    #[test]
    fn three_replicas_shuffled_gaps_converge() {
        let b1 = event(1, None);
        let b2 = event(2, Some(digest(&b1)));
        let b3 = event(3, Some(digest(&b2)));
        let events = [b1, b2, b3];
        let mut results = vec![];
        for order in [[2, 0, 1], [1, 2, 0], [0, 1, 2]] {
            let s = store();
            for i in order {
                s.ingest(&digest(&events[i]), &events[i]).unwrap();
            }
            results.push(s.states().unwrap());
        }
        assert_eq!(results[0], results[1]);
        assert_eq!(results[1], results[2]);
        assert!(results[0].values().all(|s| s == "admitted"));
        let s = store();
        assert_eq!(
            s.ingest(&digest(&events[2]), &events[2]).unwrap(),
            "pending"
        );
    }
    #[test]
    fn fork_retroactively_quarantines_descendants() {
        let a = event(1, None);
        let b = event(2, Some(digest(&a)));
        let mut other = decode(&a).unwrap();
        other.reported_state = "running".into();
        let c = serde_json::to_vec(&other).unwrap();
        let mut views = vec![];
        for order in [[&a, &b, &c], [&c, &a, &b]] {
            let s = store();
            for bytes in order {
                s.ingest(&digest(bytes), bytes).unwrap();
            }
            assert_eq!(s.export().unwrap().len(), 3);
            views.push(s.states().unwrap());
        }
        assert_eq!(views[0], views[1]);
        assert!(views[0].values().all(|s| s == "quarantined"));
    }
    #[test]
    fn duplicate_fields_and_numbers_rejected() {
        let s = store();
        let b = String::from_utf8(event(1, None)).unwrap();
        for malformed in [
            b.replacen("{", "{\"previous_event_id\":null,", 1),
            b.replacen("{", "{\"sequence\":1,", 1),
            b.replace("\"sequence\":1", "\"sequence\":1.0"),
            b.replace("\"sequence\":1", "\"sequence\":9007199254740992"),
            b.replacen("{", "{\"extra\":{},", 1),
        ] {
            assert!(s
                .ingest(&digest(malformed.as_bytes()), malformed.as_bytes())
                .is_err());
        }
        assert!(s.export().unwrap().is_empty());
    }
    #[test]
    fn wrong_scope_mesh_origin_hash_and_size_refused() {
        let s = store();
        for field in ["mesh", "origin", "scope", "type"] {
            let mut e = decode(&event(1, None)).unwrap();
            match field {
                "mesh" => e.mesh_uuid = Q.into(),
                "origin" => e.origin_replica_uuid = Q.into(),
                "scope" => e.resource_key = "host:b:workload".into(),
                _ => e.event_type = "ip.assigned".into(),
            };
            let b = serde_json::to_vec(&e).unwrap();
            assert!(s.ingest(&digest(&b), &b).is_err());
        }
        assert!(s.ingest(&"0".repeat(64), &event(1, None)).is_err());
        let big = vec![b' '; MAX_BODY + 1];
        assert!(s.ingest(&digest(&big), &big).is_err());
        assert!(s.export().unwrap().is_empty());
    }
    #[test]
    fn invalid_predecessor_is_not_admitted() {
        let s = store();
        let a = event(1, None);
        let b = event(3, Some(digest(&a)));
        s.ingest(&digest(&a), &a).unwrap();
        assert_eq!(s.ingest(&digest(&b), &b).unwrap(), "invalid");
    }
    #[test]
    fn exact_bytes_preserved_and_no_update_or_delete() {
        let s = store();
        let mut b = event(1, None);
        b.push(b'\n');
        let id = digest(&b);
        s.ingest(&id, &b).unwrap();
        assert_eq!(s.export().unwrap(), vec![(id, b)]);
        assert!(s.db.execute("DELETE FROM events", []).is_err());
        assert!(s.db.execute("UPDATE events SET seq=2", []).is_err());
    }
    #[test]
    fn durable_reopen_preserves_request_and_mesh() {
        let dir = std::env::temp_dir().join(format!(
            "podmesh-registry-{}",
            std::fs::read_to_string("/proc/sys/kernel/random/uuid")
                .unwrap()
                .trim()
        ));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("registry.sqlite");
        let b = event(1, None);
        {
            let s = Store::open(&path, M).unwrap();
            s.enroll(E, R, "host:a:").unwrap();
            s.submit(Q, &b).unwrap();
        }
        {
            let s = Store::open(&path, M).unwrap();
            assert_eq!(s.submit(Q, &b).unwrap(), digest(&b));
            assert_eq!(s.export().unwrap().len(), 1);
            assert!(Store::open(&path, Q).is_err());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn request_quota_does_not_break_identical_retry() {
        let s = store();
        let body = event(1, None);
        s.submit(Q, &body).unwrap();
        for n in 1..MAX_EVENTS {
            let request = format!("10000000-0000-4000-8000-{n:012x}");
            s.submit(&request, &body).unwrap();
        }
        assert!(s
            .submit("20000000-0000-4000-8000-000000000000", &body)
            .is_err());
        assert_eq!(s.submit(Q, &body).unwrap(), digest(&body));
        assert_eq!(s.export().unwrap().len(), 1);
    }
    #[test]
    fn cross_epoch_dependency_quarantined_but_unrelated_survives() {
        let s = store();
        s.enroll(Q, R, "host:a:").unwrap();
        let a = event(1, None);
        s.ingest(&digest(&a), &a).unwrap();
        let mut independent = decode(&a).unwrap();
        independent.producer_epoch_uuid = Q.into();
        let b = serde_json::to_vec(&independent).unwrap();
        s.ingest(&digest(&b), &b).unwrap();
        independent.sequence = 2;
        independent.previous_event_id = Some(digest(&b));
        independent.dependencies = vec![digest(&a)];
        let c = serde_json::to_vec(&independent).unwrap();
        assert_eq!(s.ingest(&digest(&c), &c).unwrap(), "admitted");
        let mut fork = decode(&a).unwrap();
        fork.reported_state = "running".into();
        let f = serde_json::to_vec(&fork).unwrap();
        s.ingest(&digest(&f), &f).unwrap();
        let states = s.states().unwrap();
        assert_eq!(states[&digest(&c)], "quarantined");
        assert_eq!(states[&digest(&b)], "admitted");
    }
}
