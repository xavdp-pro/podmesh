//! Experimental flat-file, single-writer, synchronous FUSE adapter.
//! No kernel data cache or write-back acknowledgements. VM fencing, directories,
//! hard links and general POSIX metadata persistence are intentionally absent.
use crate::{live, merkle::Inode};
use fuser::*;
use reqwest::blocking::Client;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    io,
    path::Path,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
const TTL: Duration = Duration::ZERO;
pub struct LiveFs {
    client: Client,
    peers: Vec<String>,
    volume: String,
    names: BTreeMap<u64, String>,
    next: u64,
    sequence: u64,
    session: String,
}
impl LiveFs {
    fn call(
        &self,
        method: &str,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Vec<u8>, i32> {
        // An election can outlast several immediate connection failures. Keep
        // the semantic request intact while waiting for the surviving majority.
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            for peer in &self.peers {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                let mut request = self
                    .client
                    .request(
                        if method == "POST" {
                            reqwest::Method::POST
                        } else {
                            reqwest::Method::GET
                        },
                        format!("{}/api/v1/meshfs/{path}", peer.trim_end_matches('/')),
                    )
                    .query(query)
                    .timeout(remaining.min(Duration::from_secs(5)));
                if let Some(body) = body {
                    request = request.json(body);
                }
                match request.send() {
                    Ok(r) if r.status().is_success() => {
                        // A peer may die after sending headers. Retry the same
                        // operation if its response body is incomplete as well.
                        if let Ok(bytes) = r.bytes() {
                            return Ok(bytes.to_vec());
                        }
                    }
                    Ok(r) if r.status().as_u16() == 404 => return Err(libc::ENOENT),
                    Ok(r) if matches!(r.status().as_u16(), 409 | 428) => return Err(libc::ESTALE),
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(libc::EIO)
    }
    fn inode(&self, name: &str) -> Result<Inode, i32> {
        let b = self.call(
            "GET",
            "files/inode",
            &[("volume", self.volume.clone()), ("path", name.into())],
            None,
        )?;
        serde_json::from_slice(&b).map_err(|_| libc::EIO)
    }
    fn number(&mut self, name: &str) -> u64 {
        if let Some((n, _)) = self.names.iter().find(|(_, v)| v.as_str() == name) {
            return *n;
        }
        let n = self.next;
        self.next += 1;
        self.names.insert(n, name.into());
        n
    }
    fn attr(&self, ino: u64, size: u64) -> FileAttr {
        FileAttr {
            ino,
            size,
            blocks: (size + 511) / 512,
            atime: UNIX_EPOCH,
            mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH,
            crtime: UNIX_EPOCH,
            kind: if ino == 1 {
                FileType::Directory
            } else {
                FileType::RegularFile
            },
            perm: if ino == 1 { 0o755 } else { 0o666 },
            nlink: if ino == 1 { 2 } else { 1 },
            uid: 0,
            gid: 0,
            rdev: 0,
            blksize: live::BLOCK as u32,
            flags: 0,
        }
    }
    fn metadata(&self, ino: u64) -> Result<FileAttr, i32> {
        if ino == 1 {
            return Ok(self.attr(1, 0));
        }
        let name = self.names.get(&ino).ok_or(libc::ENOENT)?;
        Ok(self.attr(ino, self.inode(name)?.size))
    }
    fn base(&self, name: &str) -> Result<live::Precondition, i32> {
        match self.inode(name) {
            Ok(i) => Ok(live::Precondition {
                version: Some(live::version(&i)),
            }),
            Err(libc::ENOENT) => Ok(live::Precondition { version: None }),
            Err(e) => Err(e),
        }
    }
    fn mutate(&mut self, name: &str, operation: live::Operation) -> Result<(), i32> {
        self.sequence += 1;
        // Capture once, before preconditions. Never rebind a timed-out mutation
        // to a newer generation; an expired request must surface ESTALE.
        let raw = self.call("GET", "files/replay-epoch", &[], None)?;
        let epoch = serde_json::from_slice::<Value>(&raw).map_err(|_| libc::EIO)?
            .get("epoch").and_then(Value::as_u64).ok_or(libc::EIO)?;
        let base = Some(self.base(name)?);
        let destination = if let live::Operation::Rename { to } = &operation {
            Some(self.base(to)?)
        } else {
            None
        };
        let body = serde_json::to_value(live::Request {
            epoch: Some(epoch),
            id: format!("{}-{}", self.session, self.sequence),
            volume: self.volume.clone(),
            path: name.into(),
            operation,
            base,
            destination,
        })
        .map_err(|_| libc::EIO)?;
        let raw = self.call("POST", "files/live", &[], Some(&body))?;
        let v: Value = serde_json::from_slice(&raw).map_err(|_| libc::EIO)?;
        if v["ok"] == true {
            Ok(())
        } else {
            Err(libc::EIO)
        }
    }
    fn barrier(&self) -> Result<(), i32> {
        let raw = self.call("GET", "volumes", &[], None)?;
        let v: Value = serde_json::from_slice(&raw).map_err(|_| libc::EIO)?;
        if v.get("error").is_some() {
            Err(libc::EIO)
        } else {
            Ok(())
        }
    }
}
impl Filesystem for LiveFs {
    fn lookup(&mut self, _: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        if parent != 1 {
            reply.error(libc::ENOTDIR);
            return;
        }
        let name = name.to_string_lossy();
        match self.inode(&name) {
            Ok(i) => {
                let n = self.number(&name);
                reply.entry(&TTL, &self.attr(n, i.size), 0)
            }
            Err(e) => reply.error(e),
        }
    }
    fn getattr(&mut self, _: &Request<'_>, ino: u64, _: Option<u64>, reply: ReplyAttr) {
        match self.metadata(ino) {
            Ok(a) => reply.attr(&TTL, &a),
            Err(e) => reply.error(e),
        }
    }
    fn open(&mut self, _: &Request<'_>, ino: u64, _: i32, reply: ReplyOpen) {
        if self.names.contains_key(&ino) {
            reply.opened(ino, 1)
        } else {
            reply.error(libc::ENOENT)
        }
    }
    fn read(
        &mut self,
        _: &Request<'_>,
        ino: u64,
        _: u64,
        offset: i64,
        size: u32,
        _: i32,
        _: Option<u64>,
        reply: ReplyData,
    ) {
        if offset < 0 {
            reply.error(libc::EINVAL);
            return;
        }
        let Some(name) = self.names.get(&ino) else {
            reply.error(libc::ENOENT);
            return;
        };
        let mut data = vec![];
        let mut at = offset as u64;
        while data.len() < (size as usize) {
            let count = (size as usize - data.len()).min(live::MAX_IO);
            match self.call(
                "GET",
                "files/range",
                &[
                    ("volume", self.volume.clone()),
                    ("path", name.clone()),
                    ("offset", at.to_string()),
                    ("length", count.to_string()),
                ],
                None,
            ) {
                Ok(b) => {
                    at += b.len() as u64;
                    let end = b.len() < count;
                    data.extend(b);
                    if end {
                        break;
                    }
                }
                Err(e) => {
                    reply.error(e);
                    return;
                }
            }
        }
        reply.data(&data)
    }
    fn write(
        &mut self,
        _: &Request<'_>,
        ino: u64,
        _: u64,
        offset: i64,
        data: &[u8],
        _: u32,
        _: i32,
        _: Option<u64>,
        reply: ReplyWrite,
    ) {
        if offset < 0 {
            reply.error(libc::EINVAL);
            return;
        }
        let Some(name) = self.names.get(&ino).cloned() else {
            reply.error(libc::ENOENT);
            return;
        };
        let mut written = 0;
        for part in data.chunks(live::MAX_IO) {
            if let Err(e) = self.mutate(
                &name,
                live::Operation::Write {
                    offset: offset as u64 + written as u64,
                    hex: hex::encode(part),
                },
            ) {
                if written > 0 {
                    reply.written(written as u32)
                } else {
                    reply.error(e)
                }
                return;
            }
            written += part.len();
        }
        reply.written(written as u32)
    }
    fn create(
        &mut self,
        _: &Request<'_>,
        parent: u64,
        name: &OsStr,
        _: u32,
        _: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
        if parent != 1 {
            reply.error(libc::ENOTDIR);
            return;
        }
        let name = name.to_string_lossy();
        match self.inode(&name) {
            Ok(_) if flags & libc::O_EXCL != 0 => {
                reply.error(libc::EEXIST);
                return;
            }
            Err(e) if e != libc::ENOENT => {
                reply.error(e);
                return;
            }
            _ => {}
        }
        match self.mutate(&name, live::Operation::Create) {
            Ok(()) => {
                let n = self.number(&name);
                match self.metadata(n) {
                    Ok(a) => reply.created(&TTL, &a, 0, n, 1),
                    Err(e) => reply.error(e),
                }
            }
            Err(e) => reply.error(e),
        }
    }
    fn setattr(
        &mut self,
        _: &Request<'_>,
        ino: u64,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        _: Option<u64>,
        crtime: Option<SystemTime>,
        chgtime: Option<SystemTime>,
        bkuptime: Option<SystemTime>,
        flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        if mode.is_some()
            || uid.is_some()
            || gid.is_some()
            || atime.is_some()
            || mtime.is_some()
            || crtime.is_some()
            || chgtime.is_some()
            || bkuptime.is_some()
            || flags.is_some()
        {
            reply.error(libc::EOPNOTSUPP);
            return;
        }
        if let Some(size) = size {
            let Some(name) = self.names.get(&ino).cloned() else {
                reply.error(libc::ENOENT);
                return;
            };
            if let Err(e) = self.mutate(&name, live::Operation::Resize { size }) {
                reply.error(e);
                return;
            }
        }
        match self.metadata(ino) {
            Ok(a) => reply.attr(&TTL, &a),
            Err(e) => reply.error(e),
        }
    }
    fn flush(&mut self, _: &Request<'_>, _: u64, _: u64, _: u64, reply: ReplyEmpty) {
        match self.barrier() {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(e),
        }
    }
    fn fsync(&mut self, _: &Request<'_>, _: u64, _: u64, _: bool, reply: ReplyEmpty) {
        match self.barrier() {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(e),
        }
    }
    fn opendir(&mut self, _: &Request<'_>, ino: u64, _: i32, reply: ReplyOpen) {
        if ino == 1 {
            reply.opened(1, 0)
        } else {
            reply.error(libc::ENOTDIR)
        }
    }
    fn readdir(
        &mut self,
        _: &Request<'_>,
        ino: u64,
        _: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        if ino != 1 {
            reply.error(libc::ENOTDIR);
            return;
        }
        let raw = match self.call("GET", "volumes", &[], None) {
            Ok(v) => v,
            Err(e) => {
                reply.error(e);
                return;
            }
        };
        let v: Value = match serde_json::from_slice(&raw) {
            Ok(v) => v,
            Err(_) => {
                reply.error(libc::EIO);
                return;
            }
        };
        let mut entries = vec![
            (1, FileType::Directory, ".".to_string()),
            (1, FileType::Directory, "..".to_string()),
        ];
        if let Some(files) = v["volumes"][&self.volume]["files"].as_object() {
            for name in files.keys() {
                let n = self.number(name);
                entries.push((n, FileType::RegularFile, name.clone()));
            }
        }
        for (j, (n, k, name)) in entries.into_iter().enumerate().skip(offset.max(0) as usize) {
            if reply.add(n, (j + 1) as i64, k, name) {
                break;
            }
        }
        reply.ok()
    }
    fn unlink(&mut self, _: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        if parent != 1 {
            reply.error(libc::ENOTDIR);
            return;
        }
        match self.mutate(&name.to_string_lossy(), live::Operation::Remove) {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(e),
        }
    }
    fn rename(
        &mut self,
        _: &Request<'_>,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        flags: u32,
        reply: ReplyEmpty,
    ) {
        if parent != 1 || newparent != 1 || flags != 0 {
            reply.error(libc::EOPNOTSUPP);
            return;
        }
        let old = name.to_string_lossy();
        let new = newname.to_string_lossy();
        match self.mutate(
            &old,
            live::Operation::Rename {
                to: new.to_string(),
            },
        ) {
            Ok(()) => {
                for n in self.names.values_mut() {
                    if n == old.as_ref() {
                        *n = new.to_string();
                    }
                }
                reply.ok()
            }
            Err(e) => reply.error(e),
        }
    }
    fn statfs(&mut self, _: &Request<'_>, _: u64, reply: ReplyStatfs) {
        reply.statfs(0, 0, 0, 0, 0, live::BLOCK as u32, 255, live::BLOCK as u32)
    }
}
pub fn mount(peers: Vec<String>, volume: String, path: &Path) -> io::Result<()> {
    if peers.is_empty() {
        return Err(io::Error::other("at least one explicit peer required"));
    }
    let fs = LiveFs {
        client: Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(io::Error::other)?,
        peers,
        volume,
        names: BTreeMap::new(),
        next: 2,
        sequence: 0,
        session: format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ),
    };
    fs.barrier()
        .map_err(|_| io::Error::other("cluster unavailable before mount"))?;
    fuser::mount2(
        fs,
        path,
        &[
            MountOption::FSName("meshfs-live-experimental".into()),
            MountOption::DefaultPermissions,
            MountOption::AllowOther,
            MountOption::NoAtime,
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    #[test]
    fn retries_election_and_lost_response_with_same_semantic_id() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let peer = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for attempt in 0..6 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut raw = Vec::new();
                let mut byte = [0];
                while !raw.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    raw.push(byte[0]);
                }
                let header = String::from_utf8(raw).unwrap();
                let length: usize = header
                    .lines()
                    .find_map(|l| {
                        l.to_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse().unwrap())
                    })
                    .unwrap();
                let mut body = vec![0; length];
                stream.read_exact(&mut body).unwrap();
                assert_eq!(
                    serde_json::from_slice::<Value>(&body).unwrap()["id"],
                    "stable-request"
                );
                if attempt == 0 {
                    stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 50\r\nConnection: close\r\n\r\n{",
                        )
                        .unwrap();
                } else if attempt < 5 {
                    stream.write_all(b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                } else {
                    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}").unwrap();
                }
            }
        });
        let fs = LiveFs {
            client: Client::new(),
            peers: vec![peer],
            volume: "test".into(),
            names: BTreeMap::new(),
            next: 2,
            sequence: 0,
            session: "test".into(),
        };
        assert_eq!(
            fs.call(
                "POST",
                "files/live",
                &[],
                Some(&serde_json::json!({"id":"stable-request"}))
            )
            .unwrap(),
            br#"{"ok":true}"#
        );
        server.join().unwrap();
    }
}
