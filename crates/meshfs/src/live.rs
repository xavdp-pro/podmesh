//! Bounded synchronous file operations for the experimental live mount.
//! Mutations publish a CAS-guarded inode only after block replication. A semantic
//! operation ID is persisted with the result so failover retries cannot reapply it.
use crate::{
    chunk::{blake3_digest, ChunkRef},
    cluster::Action,
    engine::MeshFSEngine,
    merkle::Inode,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, io};
pub const BLOCK: usize = 64 * 1024;
pub const MAX_IO: usize = 256 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Operation {
    Write { offset: u64, hex: String },
    Resize { size: u64 },
    Create,
    Remove,
    Rename { to: String },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Precondition {
    pub version: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    /// Immutable replay generation captured before reading preconditions.
    #[serde(default)]
    pub epoch: Option<u64>,
    pub id: String,
    pub volume: String,
    pub path: String,
    pub operation: Operation,
    #[serde(default)]
    pub base: Option<Precondition>,
    #[serde(default)]
    pub destination: Option<Precondition>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Commit {
    pub epoch: u64,
    pub id: String,
    pub hash: String,
    pub volume: String,
    pub path: String,
    pub expected: Option<String>,
    pub inode: Option<Inode>,
    pub rename: Option<String>,
    #[serde(default)]
    pub destination: Option<Precondition>,
    #[serde(default)]
    pub probe: bool,
}
pub fn version(i: &Inode) -> String {
    format!("{}:{}@{}", i.revision, i.digest, i.updated_at)
}
pub fn digest(size: u64, chunks: &[ChunkRef]) -> String {
    let mut h = blake3::Hasher::new();
    h.update(&size.to_be_bytes());
    for c in chunks {
        h.update(c.id.as_bytes());
        h.update(&(c.size as u64).to_be_bytes());
        h.update(&c.offset.to_be_bytes());
    }
    format!("merkle-blake3:{}", h.finalize().to_hex())
}
pub fn validate(i: &Inode, sizes: &BTreeMap<String, usize>) -> io::Result<()> {
    let mut offset = 0;
    if i.chunks_count != i.chunks.len() {
        return Err(io::Error::other("invalid chunk count"));
    }
    for (n, c) in i.chunks.iter().enumerate() {
        if (n + 1 < i.chunks.len() && c.size != BLOCK)
            || c.offset != offset
            || c.size == 0
            || c.size > BLOCK
            || sizes.get(&c.id) != Some(&c.size)
        {
            return Err(io::Error::other("invalid live block layout"));
        }
        offset += c.size as u64;
    }
    if offset != i.size || digest(i.size, &i.chunks) != i.digest {
        return Err(io::Error::other("invalid live inode digest"));
    }
    Ok(())
}
impl MeshFSEngine {
    pub async fn live_read(
        &self,
        volume: &str,
        path: &str,
        offset: u64,
        length: usize,
    ) -> io::Result<Vec<u8>> {
        if length > MAX_IO {
            return Err(io::Error::other("range exceeds live IO bound"));
        }
        let inode = self.file_inode(volume, path).await?;
        let end = offset.saturating_add(length as u64).min(inode.size);
        let mut out = Vec::new();
        for c in &inode.chunks {
            let ce = c.offset + c.size as u64;
            if ce <= offset || c.offset >= end {
                continue;
            }
            let b = self.file_chunk(&c.id).await?;
            if b.len() != c.size {
                return Err(io::Error::other("block size mismatch"));
            }
            out.extend_from_slice(
                &b[(offset.saturating_sub(c.offset)) as usize
                    ..(end - c.offset).min(c.size as u64) as usize],
            );
        }
        Ok(out)
    }
    pub async fn live_replay_epoch(&self) -> io::Result<Value> {
        self.cluster.get().ok_or_else(|| io::Error::other("cluster required"))?
            .write(Action::ReplayEpoch).await
    }
    pub async fn live_apply(&self, r: Request) -> io::Result<Value> {
        if r.id.is_empty()
            || r.id.len() > 160
            || r.path.is_empty()
            || r.path.contains('/')
            || r.path == "."
            || r.path == ".."
        {
            return Err(io::Error::other("invalid flat-volume request"));
        }
        if let Operation::Rename { to } = &r.operation {
            if to.is_empty() || to.contains('/') || to == "." || to == ".." {
                return Err(io::Error::other("invalid rename"));
            }
        }
        let cluster = self
            .cluster
            .get()
            .ok_or_else(|| io::Error::other("live access requires an explicit cluster"))?;
        let epoch = r.epoch.ok_or_else(|| io::Error::other("live replay epoch required"))?;
        let current = self.live_replay_epoch().await?["epoch"].as_u64()
            .ok_or_else(|| io::Error::other("invalid replay epoch response"))?;
        if epoch > current || current - epoch > 1 {
            return Err(io::Error::other("live replay epoch expired"));
        }
        let hash = blake3_digest(&serde_json::to_vec(&r).map_err(io::Error::other)?);
        let expected = r
            .base
            .as_ref()
            .ok_or_else(|| io::Error::other("live precondition required"))?
            .version
            .clone();
        if matches!(r.operation, Operation::Rename { .. }) && r.destination.is_none() {
            return Err(io::Error::other("rename destination precondition required"));
        }
        {
            let old = match self.file_inode(&r.volume, &r.path).await {
                Ok(v) => Some(v),
                Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                Err(e) => return Err(e),
            };
            if old.as_ref().map(version) != expected
                || (old.is_none() && !matches!(r.operation, Operation::Create))
            {
                // Consult durable semantic dedup before rejecting a retry whose
                // original precondition is no longer current. Probe cannot mutate.
                return cluster
                    .write(Action::Live(Commit {
                        epoch,
                        id: r.id.clone(),
                        hash: hash.clone(),
                        volume: r.volume.clone(),
                        path: r.path.clone(),
                        expected: expected.clone(),
                        inode: None,
                        rename: None,
                        destination: None,
                        probe: true,
                    }))
                    .await;
            }
            if matches!(r.operation, Operation::Create) && old.is_some() {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, "file exists"));
            }
            let mut inode = old.clone().unwrap_or(Inode {
                revision: 0,
                path: r.path.clone(),
                size: 0,
                digest: digest(0, &[]),
                chunks_count: 0,
                chunks: vec![],
                updated_at: String::new(),
            });
            let mut rename = None;
            let mut remove = false;
            match &r.operation {
                Operation::Create => {}
                Operation::Remove => remove = true,
                Operation::Rename { to } => rename = Some(to.clone()),
                Operation::Write { offset, hex } => {
                    let bytes = hex::decode(hex).map_err(io::Error::other)?;
                    if bytes.len() > MAX_IO {
                        return Err(io::Error::other("write exceeds IO bound"));
                    }
                    let end = offset
                        .checked_add(bytes.len() as u64)
                        .ok_or_else(|| io::Error::other("offset overflow"))?;
                    if bytes.is_empty() {
                        return Ok(json!({"ok":true}));
                    }
                    let new_size = inode.size.max(end);
                    self.live_resize_blocks(&mut inode, new_size).await?;
                    for index in (*offset / BLOCK as u64) as usize
                        ..((end + BLOCK as u64 - 1) / BLOCK as u64) as usize
                    {
                        let c = &inode.chunks[index];
                        let mut block = self.file_chunk(&c.id).await?;
                        let from = (*offset).max(c.offset);
                        let to = end.min(c.offset + c.size as u64);
                        block[(from - c.offset) as usize..(to - c.offset) as usize]
                            .copy_from_slice(
                                &bytes[(from - *offset) as usize..(to - *offset) as usize],
                            );
                        let id = self.live_stage(&block).await?;
                        inode.chunks[index].id = id;
                    }
                }
                Operation::Resize { size } => self.live_resize_blocks(&mut inode, *size).await?,
            }
            inode.digest = digest(inode.size, &inode.chunks);
            inode.updated_at = chrono::Utc::now().to_rfc3339();
            let commit = Commit {
                epoch,
                id: r.id.clone(),
                hash: hash.clone(),
                volume: r.volume.clone(),
                path: r.path.clone(),
                expected,
                inode: if remove || rename.is_some() {
                    None
                } else {
                    Some(inode)
                },
                rename,
                destination: r.destination.clone(),
                probe: false,
            };
            return cluster.write(Action::Live(commit)).await;
        }
    }
    async fn live_stage(&self, bytes: &[u8]) -> io::Result<String> {
        let c = self
            .cluster
            .get()
            .ok_or_else(|| io::Error::other("cluster required"))?;
        serde_json::from_value(
            c.write(Action::StageChunk {
                hex: hex::encode(bytes),
            })
            .await?,
        )
        .map_err(io::Error::other)
    }
    async fn live_resize_blocks(&self, i: &mut Inode, size: u64) -> io::Result<()> {
        if !i.digest.starts_with("merkle-blake3:") && !i.digest.starts_with("merkle-sha256:") {
            return Err(io::Error::other(
                "import the file through the live interface first",
            ));
        }
        if size > 262144 * BLOCK as u64 {
            return Err(io::Error::other("experimental live size bound exceeded"));
        }
        let count = size.div_ceil(BLOCK as u64) as usize;
        if count > 262144 {
            return Err(io::Error::other(
                "experimental live manifest bound exceeded",
            ));
        }
        i.chunks.truncate(count);
        for n in 0..count {
            let len = ((size - n as u64 * BLOCK as u64).min(BLOCK as u64)) as usize;
            if let Some(c) = i.chunks.get(n) {
                if c.size == len {
                    continue;
                }
            }
            let mut bytes = if let Some(c) = i.chunks.get(n) {
                self.file_chunk(&c.id).await?
            } else {
                vec![]
            };
            bytes.resize(len, 0);
            let c = ChunkRef {
                id: self.live_stage(&bytes).await?,
                size: len,
                offset: n as u64 * BLOCK as u64,
            };
            if n < i.chunks.len() {
                i.chunks[n] = c
            } else {
                i.chunks.push(c)
            }
        }
        i.size = size;
        i.chunks_count = i.chunks.len();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manifest_rejects_reordered_missing_and_corrupt_blocks() {
        let id = "blake3:example".to_string();
        let chunks = vec![
            ChunkRef {
                id: id.clone(),
                size: BLOCK,
                offset: 0,
            },
            ChunkRef {
                id: id.clone(),
                size: BLOCK,
                offset: BLOCK as u64,
            },
        ];
        let sizes = BTreeMap::from([(id, BLOCK)]);
        let mut i = Inode {
            revision: 0,
            path: "disk.raw".into(),
            size: (BLOCK * 2) as u64,
            digest: digest((BLOCK * 2) as u64, &chunks),
            chunks_count: 2,
            chunks,
            updated_at: "now".into(),
        };
        assert!(validate(&i, &sizes).is_ok());
        i.chunks[1].offset = 0;
        assert!(validate(&i, &sizes).is_err());
        i.chunks[1].offset = BLOCK as u64;
        i.digest.push('x');
        assert!(validate(&i, &sizes).is_err());
        i.digest = digest(i.size, &i.chunks);
        assert!(validate(&i, &BTreeMap::new()).is_err());
    }
}
