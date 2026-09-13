//! Transport-neutral laboratory batches. The caller must authenticate peers.
use crate::{digest, hash, Result, Store, MAX_BODY, MAX_EVENTS};
use serde::{Deserialize, Serialize};

pub const MAX_BATCH_EVENTS: usize = 64;
pub const MAX_BATCH_BYTES: usize = 256 * 1024;
pub const MAX_WIRE_BYTES: usize = 600 * 1024;
// Every legal event fits and pagination must make progress. Metadata contains only
// bounded ASCII fields: allow 128 bytes/record plus 2048 bytes for the envelope.
const _: () = assert!(MAX_BATCH_EVENTS > 0 && MAX_BATCH_BYTES >= MAX_BODY);
const _: () = assert!(2 * MAX_BATCH_BYTES + 128 * MAX_BATCH_EVENTS + 2048 <= MAX_WIRE_BYTES);
const FORMAT: &str = "podmesh-observation-exchange-lab/1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Cursor {
    pub snapshot: String,
    pub offset: usize,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    id: String,
    hex: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Batch {
    format: String,
    mesh: String,
    total: usize,
    cursor: Cursor,
    next: Option<Cursor>,
    records: Vec<Record>,
}
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    Validation,
    Enrollment,
    Quota,
    RetryableStorage,
    StorageFault,
}
fn classify(error: &(dyn std::error::Error + 'static)) -> Refusal {
    if let Some(e) = error.downcast_ref::<rusqlite::Error>() {
        return match e {
            rusqlite::Error::QueryReturnedNoRows => Refusal::Enrollment,
            rusqlite::Error::SqliteFailure(code, _)
                if matches!(
                    code.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                ) =>
            {
                Refusal::RetryableStorage
            }
            _ => Refusal::StorageFault,
        };
    }
    match error.to_string().as_str() {
        "lab retained-history quota exhausted; no automatic eviction"
        | "lab history quota exhausted; no automatic eviction" => Refusal::Quota,
        "origin or scope mismatch" => Refusal::Enrollment,
        _ if error.is::<serde_json::Error>() => Refusal::Validation,
        "event too large"
        | "hash mismatch"
        | "invalid observation envelope"
        | "wrong mesh or self dependency" => Refusal::Validation,
        _ => Refusal::StorageFault,
    }
}
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventReceipt {
    pub id: String,
    /// Storage acceptance is distinct from replay admission or runtime truth.
    pub stored: bool,
    pub admission: Option<String>,
    pub refusal: Option<Refusal>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Receipt {
    /// Binds this result to the exact batch bytes, including its cursor.
    pub batch_digest: String,
    pub events: Vec<EventReceipt>,
}
impl Receipt {
    /// A cursor is progress only after every exact event has a storage receipt.
    /// Authentication of this receipt remains the transport caller's duty.
    pub fn advance(&self, wire: &[u8]) -> Result<Option<Cursor>> {
        if wire.len() > MAX_WIRE_BYTES || self.batch_digest != digest(wire) {
            return Err("receipt batch mismatch".into());
        }
        let batch: Batch = serde_json::from_slice(wire)?;
        if self.events.len() != batch.records.len()
            || !self
                .events
                .iter()
                .zip(&batch.records)
                .all(|(receipt, record)| receipt.id == record.id && receipt.stored)
        {
            return Err("batch incomplete; retain cursor and reconcile refusals".into());
        }
        Ok(batch.next)
    }
}
/// Frozen export: cursors cannot skip new events inserted before an old digest.
pub struct Snapshot {
    mesh: String,
    id: String,
    records: Vec<(String, Vec<u8>)>,
}
impl Snapshot {
    pub fn capture(store: &Store) -> Result<Self> {
        let records = store.export()?;
        if records.len() > MAX_EVENTS || records.iter().any(|(_, b)| b.len() > MAX_BODY) {
            return Err("stored export violates laboratory bounds".into());
        }
        let ids: Vec<_> = records.iter().map(|(id, _)| id).collect();
        let id = digest(&serde_json::to_vec(&(&store.mesh, ids))?);
        Ok(Self {
            mesh: store.mesh.clone(),
            id,
            records,
        })
    }
    pub fn page(&self, cursor: Option<&Cursor>) -> Result<(Vec<u8>, Option<Cursor>)> {
        let offset = match cursor {
            Some(c) if c.snapshot == self.id && c.offset < self.records.len() => c.offset,
            Some(_) => return Err("invalid or stale snapshot cursor".into()),
            None => 0,
        };
        let mut records = Vec::new();
        let mut bytes = 0;
        for (id, body) in self.records.iter().skip(offset).take(MAX_BATCH_EVENTS) {
            if bytes + body.len() > MAX_BATCH_BYTES {
                break;
            }
            bytes += body.len();
            let hex = body.iter().map(|v| format!("{v:02x}")).collect();
            records.push(Record {
                id: id.clone(),
                hex,
            });
        }
        if offset < self.records.len() && records.is_empty() {
            return Err("page cannot make progress".into());
        }
        let end = offset + records.len();
        let next = (end < self.records.len()).then(|| Cursor {
            snapshot: self.id.clone(),
            offset: end,
        });
        let batch = Batch {
            format: FORMAT.into(),
            mesh: self.mesh.clone(),
            total: self.records.len(),
            cursor: Cursor {
                snapshot: self.id.clone(),
                offset,
            },
            next: next.clone(),
            records,
        };
        let wire = serde_json::to_vec(&batch)?;
        if wire.len() > MAX_WIRE_BYTES {
            return Err("batch wire quota exceeded".into());
        }
        Ok((wire, next))
    }
}

/// Import is deliberately per-event, not batch-atomic. Lost receipts are retried
/// with identical bytes. Envelope/framing validation finishes before any write.
/// No transport identity or writer enrollment is inferred from this payload.
pub fn import(store: &Store, wire: &[u8]) -> Result<Receipt> {
    if wire.len() > MAX_WIRE_BYTES {
        return Err("batch wire quota exceeded".into());
    }
    let b: Batch = serde_json::from_slice(wire)?;
    if b.format != FORMAT
        || b.mesh != store.mesh
        || b.total > MAX_EVENTS
        || !hash(&b.cursor.snapshot)
        || b.records.len() > MAX_BATCH_EVENTS
        || b.cursor.offset > MAX_EVENTS
        || b.cursor.offset + b.records.len() > b.total
        || (b.cursor.offset + b.records.len() < b.total) != b.next.is_some()
        || b.next.as_ref().is_some_and(|n| {
            n.snapshot != b.cursor.snapshot
                || n.offset != b.cursor.offset + b.records.len()
                || b.records.is_empty()
        })
    {
        return Err("invalid batch envelope".into());
    }
    let mut decoded = Vec::new();
    let mut bytes = 0;
    let mut previous: Option<&str> = None;
    for r in &b.records {
        if !hash(&r.id)
            || previous.is_some_and(|p| p >= r.id.as_str())
            || r.hex.len() % 2 != 0
            || r.hex.len() > MAX_BODY * 2
            || !r
                .hex
                .bytes()
                .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
        {
            return Err("invalid batch record encoding".into());
        }
        previous = Some(&r.id);
        let body: Vec<u8> = r
            .hex
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                let n = |v: u8| if v <= b'9' { v - b'0' } else { v - b'a' + 10 };
                n(pair[0]) * 16 + n(pair[1])
            })
            .collect();
        bytes += body.len();
        if bytes > MAX_BATCH_BYTES {
            return Err("batch body quota exceeded".into());
        }
        decoded.push((&r.id, body));
    }
    let mut events = Vec::new();
    for (id, body) in decoded {
        let result = store.ingest(id, &body);
        // Query storage separately: ingest may fail after commit during replay.
        let stored: bool = store.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE id=?1 AND body=?2)",
            rusqlite::params![id, body],
            |r| r.get(0),
        )?;
        events.push(EventReceipt {
            id: id.clone(),
            stored,
            refusal: result.as_ref().err().map(|e| classify(e.as_ref())),
            admission: result.ok(),
        });
    }
    Ok(Receipt {
        batch_digest: digest(wire),
        events,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wire_bound_covers_max_body_and_record_metadata() {
        let s = Snapshot {
            mesh: "0".repeat(36),
            id: "a".repeat(64),
            records: (0..64)
                .map(|n| (format!("{n:064x}"), vec![b' '; 4096]))
                .collect(),
        };
        let (wire, next) = s.page(None).unwrap();
        assert!(wire.len() <= 2 * MAX_BATCH_BYTES + 128 * MAX_BATCH_EVENTS + 2048);
        assert!(next.is_none());
    }
    #[test]
    fn invalid_internal_oversize_event_cannot_produce_self_cursor() {
        let s = Snapshot {
            mesh: "0".repeat(36),
            id: "a".repeat(64),
            records: vec![("b".repeat(64), vec![0; MAX_BATCH_BYTES + 1])],
        };
        assert!(s.page(None).is_err());
    }
    #[test]
    fn refusal_classes_do_not_expose_sqlite_details() {
        let busy = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(5),
            Some("private path".into()),
        );
        assert_eq!(classify(&busy), Refusal::RetryableStorage);
        let full = rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(13), None);
        assert_eq!(classify(&full), Refusal::StorageFault);
        let quota: Box<dyn std::error::Error> =
            "lab retained-history quota exhausted; no automatic eviction".into();
        assert_eq!(classify(quota.as_ref()), Refusal::Quota);
        let bad: Box<dyn std::error::Error> = "hash mismatch".into();
        assert_eq!(classify(bad.as_ref()), Refusal::Validation);
    }
}
