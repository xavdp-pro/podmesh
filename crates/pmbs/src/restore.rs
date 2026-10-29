//! Linux regular-content restore, using server metadata and authenticated chunk GETs.
//! No workload execution or file-attribute restoration. Trusted local administrator
//! boundary: directory descriptors prevent symlink traversal; hostile same-UID/root
//! replacement of our private staging tree is outside that boundary.
use pmbs::cli::RestoreArgs;
use pmbs::datastore::{validate_chunk_id, validate_file_path, validate_name};
use pmbs::server::configured_token;
use pmbs::{BackupManifest, CHUNK_SIZE};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::ffi::{CString, OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

const METADATA_LIMIT: usize = 32 * 1024 * 1024;
#[derive(Deserialize)]
struct SnapshotList {
    ok: bool,
    snapshots: Vec<BackupManifest>,
}
fn invalid(message: &str) -> String { message.to_owned() }
fn io_error(e: io::Error) -> String { e.to_string() }
fn cstring(name: &OsStr) -> io::Result<CString> {
    CString::new(name.as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))
}
fn open_dir_at(parent: &File, name: &OsStr) -> io::Result<File> {
    let name = cstring(name)?;
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if fd < 0 { return Err(io::Error::last_os_error()); }
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn mkdir_at(parent: &File, name: &OsStr) -> io::Result<()> {
    let name = cstring(name)?;
    if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } < 0 {
        return Err(io::Error::last_os_error());
    }
    parent.sync_all()
}
fn exists_at(parent: &File, name: &OsStr) -> io::Result<bool> {
    let name = cstring(name)?;
    let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstatat(parent.as_raw_fd(), name.as_ptr(), st.as_mut_ptr(), libc::AT_SYMLINK_NOFOLLOW) } == 0 { return Ok(true); }
    let e = io::Error::last_os_error();
    if e.kind() == io::ErrorKind::NotFound { Ok(false) } else { Err(e) }
}
/// Anchor every ancestor, including the destination parent, without following links.
fn destination_parent(destination: &Path) -> io::Result<(File, OsString, PathBuf)> {
    if destination.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "destination must not contain '..'"));
    }
    let absolute = if destination.is_absolute() { destination.to_owned() } else { std::env::current_dir()?.join(destination) };
    if absolute.to_str().is_none() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "destination must be valid UTF-8 for the JSON receipt"));
    }
    let mut components = absolute.components().filter_map(|c| match c { Component::Normal(n) => Some(n.to_owned()), _ => None }).collect::<Vec<_>>();
    let name = components.pop().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "destination must name a new directory"))?;
    let mut parent = File::open("/")?;
    for component in components { parent = open_dir_at(&parent, &component)?; }
    if exists_at(&parent, &name)? { return Err(io::Error::new(io::ErrorKind::AlreadyExists, "destination already exists (including symlinks)")); }
    Ok((parent, name, absolute))
}
struct Staging {
    parent: File,
    root: File,
    name: OsString,
    destination_name: OsString,
    destination: PathBuf,
    path: PathBuf,
}
impl Staging {
    fn new(destination: &Path) -> io::Result<Self> {
        let (parent, destination_name, destination) = destination_parent(destination)?;
        let mut random = [0u8; 16];
        File::open("/dev/urandom")?.read_exact(&mut random)?;
        let name = OsString::from(format!(".pmbs-restore-{}", hex::encode(random)));
        let path = destination.with_file_name(&name);
        mkdir_at(&parent, &name).map_err(|e| io::Error::new(e.kind(), format!("staging creation failed; retain if created {}: {}", path.display(), e)))?;
        let root = open_dir_at(&parent, &name).map_err(|e| io::Error::new(e.kind(), format!("staging retained {}: {}", path.display(), e)))?;
        Ok(Self { parent, root, name, destination_name, destination, path })
    }
    fn new_file(&self, logical: &str) -> io::Result<(File, File)> {
        // Callers validate first; retain the policy at the filesystem boundary too.
        validate_file_path(logical)?;
        let mut parts = logical.split('/').collect::<Vec<_>>();
        let name = parts.pop().expect("validated nonempty logical path");
        let mut dir = self.root.try_clone()?;
        for part in parts {
            match mkdir_at(&dir, OsStr::new(part)) {
                Ok(()) => (),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(e),
            }
            dir = open_dir_at(&dir, OsStr::new(part))?;
        }
        let name = cstring(OsStr::new(name))?;
        let fd = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC, 0o600) };
        if fd < 0 { return Err(io::Error::last_os_error()); }
        Ok((unsafe { File::from_raw_fd(fd) }, dir))
    }
    fn publish(&self) -> io::Result<()> {
        self.root.sync_all()?;
        let old = cstring(&self.name)?;
        let new = cstring(&self.destination_name)?;
        #[cfg(target_os = "linux")]
        let rc = unsafe { libc::renameat2(self.parent.as_raw_fd(), old.as_ptr(), self.parent.as_raw_fd(), new.as_ptr(), libc::RENAME_NOREPLACE) };
        #[cfg(not(target_os = "linux"))]
        let rc = { let _ = (old, new); return Err(io::Error::new(io::ErrorKind::Unsupported, "restore publication requires Linux renameat2")); };
        if rc < 0 { return Err(io::Error::last_os_error()); }
        self.parent.sync_all().map_err(|e| io::Error::new(e.kind(), format!("destination published but parent sync failed; keep {}: {}", self.destination.display(), e)))
    }
    // Deliberately no Drop/remove_dir_all: retain only this exclusively created
    // private tree on failure. Never delete unknown partial objects by pathname.
}
fn endpoint(remote: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(remote).map_err(|_| invalid("invalid PMBS endpoint"))?;
    let loopback = url.host_str().and_then(|host| {
        host.trim_start_matches('[').trim_end_matches(']').parse::<std::net::IpAddr>().ok()
    }).map(|ip| ip.is_loopback()).unwrap_or(false);
    if !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some()
        || url.path() != "/" || !(url.scheme() == "https" || (url.scheme() == "http" && loopback)) {
        return Err(invalid("endpoint requires HTTPS or numeric loopback HTTP, no credentials/path/query/fragment"));
    }
    Ok(url)
}
fn validate_manifest(m: &BackupManifest, max_bytes: u64) -> Result<(), String> {
    validate_name(&m.id).map_err(io_error)?;
    validate_name(&m.backup_type).map_err(io_error)?;
    chrono::DateTime::parse_from_rfc3339(&m.backup_time).map_err(|_| invalid("invalid backup timestamp"))?;
    // Persisted verification flags are informational; they never replace our checks.
    if m.files.len() > 10000 || m.total_logical_bytes > max_bytes { return Err(invalid("snapshot exceeds configured restore limits")); }
    let paths = m.files.keys().cloned().collect::<BTreeSet<_>>();
    let mut bytes = 0u64;
    let mut count = 0usize;
    for (key, f) in &m.files {
        validate_file_path(key).map_err(io_error)?;
        for (idx, _) in key.match_indices('/') { if paths.contains(&key[..idx]) { return Err(invalid("file/directory prefix collision")); } }
        if key != &f.path || f.chunks_count != f.chunks.len() { return Err(invalid("file identity/chunk count mismatch")); }
        if !f.digest.starts_with("blake3:") { return Err(invalid("file digest must be BLAKE3")); }
        validate_chunk_id(&f.digest).map_err(io_error)?;
        let mut offset = 0u64;
        for (idx, c) in f.chunks.iter().enumerate() {
            validate_chunk_id(&c.id).map_err(io_error)?;
            if c.offset != offset || c.size == 0 || c.size > CHUNK_SIZE || (idx + 1 < f.chunks.len() && c.size != CHUNK_SIZE) { return Err(invalid("invalid canonical chunk layout")); }
            offset = offset.checked_add(c.size as u64).ok_or_else(|| invalid("file size overflow"))?;
        }
        if offset != f.size { return Err(invalid("file size/layout mismatch")); }
        bytes = bytes.checked_add(f.size).ok_or_else(|| invalid("snapshot size overflow"))?;
        count = count.checked_add(f.chunks.len()).ok_or_else(|| invalid("snapshot chunk count overflow"))?;
    }
    if bytes != m.total_logical_bytes || count != m.chunks_count { return Err(invalid("snapshot aggregate mismatch")); }
    Ok(())
}
async fn get_limited(client: &reqwest::Client, url: reqwest::Url, limit: usize) -> Result<Vec<u8>, String> {
    let mut response = client.get(url).timeout(Duration::from_secs(30)).send().await.map_err(|e| format!("PMBS GET failed: {e}"))?;
    // error_for_status alone would accept redirects. Never interpret 3xx as payload.
    if !response.status().is_success() { return Err(format!("PMBS GET returned {}", response.status())); }
    if response.content_length().map(|n| n > limit as u64).unwrap_or(false) { return Err(invalid("PMBS response exceeds size limit")); }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| format!("PMBS body failed: {e}"))? {
        if chunk.len() > limit.saturating_sub(bytes.len()) { return Err(invalid("PMBS response exceeds size limit")); }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
// Reuse the existing token policy. Restore deliberately disables ambient proxies:
// numeric-loopback HTTP must never relay the administrative bearer elsewhere.
fn restore_client(token: &str) -> Result<reqwest::Client, String> {
    let mut headers = reqwest::header::HeaderMap::new();
    let mut authorization = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| invalid("invalid bearer header"))?;
    authorization.set_sensitive(true);
    headers.insert(reqwest::header::AUTHORIZATION, authorization);
    reqwest::Client::builder().default_headers(headers)
        .redirect(reqwest::redirect::Policy::none()).no_proxy().build()
        .map_err(|_| invalid("could not construct restore HTTP client"))
}
pub async fn run(args: &RestoreArgs) -> Result<Value, String> {
    let token = configured_token()?; // No silent anonymous fallback.
    restore(args, &restore_client(&token)?).await
}
async fn restore(args: &RestoreArgs, client: &reqwest::Client) -> Result<Value, String> {
    validate_name(&args.datastore).map_err(io_error)?;
    validate_name(&args.snapshot).map_err(io_error)?;
    validate_name(&args.backup_type).map_err(io_error)?;
    // Refuse an existing destination before any HTTP, without creating directories.
    destination_parent(&args.destination).map_err(io_error)?;
    let base = endpoint(&args.remote)?;
    let ds_path = format!("api/v1/datastores/{}/", args.datastore);
    let response = get_limited(client, base.join(&(ds_path.clone() + "snapshots")).map_err(|e| e.to_string())?, METADATA_LIMIT).await?;
    let listing: SnapshotList = serde_json::from_slice(&response).map_err(|e| format!("invalid server snapshot metadata: {e}"))?;
    if !listing.ok { return Err(invalid("server snapshot listing failed")); }
    let mut matches = listing.snapshots.into_iter().filter(|m| m.id == args.snapshot && m.backup_type == args.backup_type);
    let manifest = matches.next().ok_or_else(|| invalid("requested server snapshot identity not found"))?;
    if matches.next().is_some() { return Err(invalid("ambiguous server snapshot identity")); }
    validate_manifest(&manifest, args.max_bytes)?;
    let stage = Staging::new(&args.destination).map_err(io_error)?;
    let result: Result<Value, String> = async {
        let mut files = Vec::new();
        let mut names = manifest.files.keys().collect::<Vec<_>>();
        names.sort();
        for path in names {
            let f = &manifest.files[path];
            let (mut output, dir) = stage.new_file(path).map_err(io_error)?;
            let mut b3_hash = blake3::Hasher::new();
            for chunk in &f.chunks {
                let url = base.join(&format!("{}chunks/{}", ds_path, chunk.id)).map_err(|e| e.to_string())?;
                let payload = get_limited(client, url, chunk.size).await?;
                let clean_chunk = pmbs::datastore::Datastore::normalize_id(&chunk.id);
                if payload.len() != chunk.size || !pmbs::datastore::verify_cas_digest(&payload, clean_chunk) { return Err(invalid("referenced chunk length/digest mismatch")); }
                output.write_all(&payload).map_err(io_error)?;
                b3_hash.update(&payload);
            }
            let digest = format!("blake3:{}", b3_hash.finalize().to_hex());
            if output.metadata().map_err(io_error)?.len() != f.size || digest != f.digest { return Err(invalid("restored file length/digest mismatch")); }
            output.sync_all().map_err(io_error)?;
            dir.sync_all().map_err(io_error)?;
            files.push(json!({"path": path, "bytes": f.size, "digest": digest}));
        }
        stage.publish().map_err(io_error)?;
        Ok(json!({"ok": true, "snapshot": manifest.id, "backupType": manifest.backup_type, "datastore": args.datastore,
            "destination": stage.destination, "files": files, "bytes": manifest.total_logical_bytes,
            "metadataSource": "authenticated PMBS server snapshot GET", "integrityVerified": true,
            "op10SandboxBootProof": false, "bootReadiness": "NOT_TESTED",
            "scope": "regular file contents only; no ownership/mode/timestamps/ACL/xattrs/symlinks or workload execution"}))
    }.await;
    result.map_err(|e| format!("{}; private staging retained if unpublished: {}; destination: {}", e, stage.path.display(), stage.destination.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pmbs::{BackupFile, ChunkRef};
    use std::collections::HashMap;
    use std::fs;
    use std::os::unix::fs::{DirBuilderExt, symlink};
    use axum::{body::Body, extract::State, http::{Request, StatusCode}, response::{IntoResponse, Response}, routing::get, Router};
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let mut random = [0u8; 16];
            File::open("/dev/urandom").unwrap().read_exact(&mut random).unwrap();
            let p = std::env::temp_dir().canonicalize().unwrap().join(format!("pmbs-restore-test-{}", hex::encode(random)));
            fs::DirBuilder::new().mode(0o700).create(&p).unwrap(); Self(p)
        }
    }
    impl Drop for Temp { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
    fn manifest(payloads: &[Vec<u8>]) -> BackupManifest {
        let mut full = Vec::new(); let mut chunks = Vec::new();
        for bytes in payloads {
            chunks.push(ChunkRef { id: format!("blake3:{}", blake3::hash(bytes).to_hex()), offset: full.len() as u64, size: bytes.len() });
            full.extend(bytes);
        }
        let f = BackupFile { path: "nested/file".into(), size: full.len() as u64, digest: format!("blake3:{}", blake3::hash(&full).to_hex()), chunks_count: chunks.len(), chunks };
        BackupManifest { id: "snapshot".into(), backup_type: "podman".into(), backup_time: "2026-10-22T00:00:00Z".into(), files: HashMap::from([(f.path.clone(), f)]), total_logical_bytes: full.len() as u64, chunks_count: payloads.len(), verified: false, verified_at: None }
    }
    #[test]
    fn manifest_layout_and_path_policy_rejects_before_writes() {
        let valid = manifest(&[vec![1; CHUNK_SIZE], vec![2; 3]]);
        validate_manifest(&valid, u64::MAX).unwrap();
        for path in ["../escape", "/absolute", "nested//file", "a/./b", "a\\b", "a:b"] {
            let mut bad = valid.clone(); let mut f = bad.files.remove("nested/file").unwrap(); f.path = path.into(); bad.files.insert(path.into(), f);
            assert!(validate_manifest(&bad, u64::MAX).is_err(), "{path}");
        }
        let mut bad = valid.clone(); bad.files.get_mut("nested/file").unwrap().chunks[1].offset += 1;
        assert!(validate_manifest(&bad, u64::MAX).is_err());
        let mut bad = valid.clone(); bad.files.get_mut("nested/file").unwrap().chunks[0].size -= 1;
        assert!(validate_manifest(&bad, u64::MAX).is_err());
        let mut bad = valid.clone(); bad.total_logical_bytes += 1;
        assert!(validate_manifest(&bad, u64::MAX).is_err());
        assert!(validate_manifest(&valid, 1).is_err());
        let mut bad = valid.clone(); let mut prefix = bad.files["nested/file"].clone(); prefix.path = "nested".into(); bad.files.insert("nested".into(), prefix);
        assert!(validate_manifest(&bad, u64::MAX).unwrap_err().contains("prefix collision"));
    }
    #[test]
    fn destination_and_publication_never_replace_existing_objects() {
        let t = Temp::new(); let dst = t.0.join("output");
        fs::create_dir(&dst).unwrap(); assert!(Staging::new(&dst).is_err()); fs::remove_dir(&dst).unwrap();
        symlink(t.0.join("absent"), &dst).unwrap(); assert!(Staging::new(&dst).is_err()); fs::remove_file(&dst).unwrap();
        let staging = Staging::new(&dst).unwrap();
        fs::create_dir(&dst).unwrap(); fs::write(dst.join("foreign"), b"keep").unwrap();
        assert!(staging.publish().is_err()); assert_eq!(fs::read(dst.join("foreign")).unwrap(), b"keep");
        assert!(staging.path.is_dir());
    }
    #[test]
    fn ancestor_and_internal_symlinks_cannot_escape() {
        let t = Temp::new(); let outside = Temp::new();
        symlink(&outside.0, t.0.join("linked-parent")).unwrap();
        assert!(Staging::new(&t.0.join("linked-parent/output")).is_err());
        let staging = Staging::new(&t.0.join("output")).unwrap();
        symlink(&outside.0, staging.path.join("trap")).unwrap();
        assert!(staging.new_file("trap/escaped").is_err()); assert!(!outside.0.join("escaped").exists());
        assert!(staging.new_file("../escaped").is_err());
    }
    #[test]
    fn endpoint_refuses_cleartext_remote_and_embedded_credentials() {
        for url in ["http://example.com", "http://localhost:8007", "https://user:secret@example.com", "https://example.com/other", "https://example.com/?x=1", "ftp://127.0.0.1"] { assert!(endpoint(url).is_err(), "{url}"); }
        for url in ["http://127.0.0.1:8007", "http://[::1]:8007", "https://example.com"] { endpoint(url).unwrap(); }
    }
    #[derive(Clone)]
    struct Mock {
        manifest: BackupManifest,
        payloads: Arc<HashMap<String, Vec<u8>>>,
        mode: Arc<AtomicUsize>, // 0 good, 1 corrupt, 2 missing, 3 redirect, 4 oversize
        calls: Arc<AtomicUsize>,
    }
    const TOKEN: &str = "test-only-token-012345678901234567890";
    async fn serve(State(state): State<Mock>, request: Request<Body>) -> Response {
        state.calls.fetch_add(1, Ordering::Relaxed);
        let expected_auth = format!("Bearer {TOKEN}");
        if request.headers().get("authorization").and_then(|v| v.to_str().ok()) != Some(expected_auth.as_str()) { return StatusCode::UNAUTHORIZED.into_response(); }
        let path = request.uri().path();
        if path.ends_with("/snapshots") {
            if state.mode.load(Ordering::Relaxed) == 3 { return (StatusCode::TEMPORARY_REDIRECT, [("location", "/must-not-follow")]).into_response(); }
            return axum::Json(json!({"ok": true, "snapshots": [state.manifest]})).into_response();
        }
        if let Some(id) = path.split("/chunks/").nth(1) {
            let bytes = state.payloads.get(id).cloned().unwrap_or_default();
            return match state.mode.load(Ordering::Relaxed) {
                1 => vec![0; bytes.len()].into_response(),
                2 => StatusCode::NOT_FOUND.into_response(),
                4 => vec![0; CHUNK_SIZE + 1].into_response(),
                _ => bytes.into_response(),
            };
        }
        StatusCode::NOT_FOUND.into_response()
    }
    struct MockServer(tokio::task::JoinHandle<()>);
    impl Drop for MockServer { fn drop(&mut self) { self.0.abort(); } }
    async fn start_mock() -> (MockServer, Mock, String) {
        let payloads = vec![vec![7; CHUNK_SIZE], b"tail".to_vec()];
        let mut m = manifest(&payloads);
        let empty = BackupFile { path: "empty".into(), size: 0, digest: format!("blake3:{}", blake3::hash(b"").to_hex()), chunks_count: 0, chunks: Vec::new() };
        m.files.insert("empty".into(), empty);
        let state = Mock { manifest: m, payloads: Arc::new(payloads.into_iter().map(|b| (format!("blake3:{}", blake3::hash(&b).to_hex()), b)).collect()), mode: Arc::new(AtomicUsize::new(0)), calls: Arc::new(AtomicUsize::new(0)) };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().fallback(get(serve)).with_state(state.clone());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        (MockServer(task), state, format!("http://{addr}"))
    }
    fn args(t: &Temp, remote: String) -> RestoreArgs {
        RestoreArgs { destination: t.0.join("output"), datastore: "store".into(), snapshot: "snapshot".into(), backup_type: "podman".into(), remote, max_bytes: 2 * CHUNK_SIZE as u64 }
    }
    fn client(token: &str) -> reqwest::Client { restore_client(token).unwrap() }
    #[tokio::test]
    async fn authenticated_server_metadata_multichunk_and_empty_file_restore() {
        let (_server, state, remote) = start_mock().await;
        let t = Temp::new(); let a = args(&t, remote);
        let report = restore(&a, &client(TOKEN)).await.unwrap();
        let bytes = fs::read(a.destination.join("nested/file")).unwrap();
        assert_eq!(&bytes[..CHUNK_SIZE], vec![7; CHUNK_SIZE]); assert_eq!(&bytes[CHUNK_SIZE..], b"tail");
        assert!(fs::read(a.destination.join("empty")).unwrap().is_empty());
        assert_eq!(report["bytes"], CHUNK_SIZE as u64 + 4); assert_eq!(report["bootReadiness"], "NOT_TESTED"); assert_eq!(report["op10SandboxBootProof"], false);
        assert_eq!(state.calls.load(Ordering::Relaxed), 3);
        assert!(restore(&a, &client(TOKEN)).await.is_err());
        assert_eq!(state.calls.load(Ordering::Relaxed), 3); // existing target refused before HTTP
    }
    #[tokio::test]
    async fn auth_redirect_and_snapshot_identity_fail_without_destination() {
        let (_server, state, remote) = start_mock().await;
        let t = Temp::new(); let mut a = args(&t, remote);
        assert!(restore(&a, &client("wrong")).await.unwrap_err().contains("401"));
        a.backup_type = "wrong-type".into();
        assert!(restore(&a, &client(TOKEN)).await.unwrap_err().contains("identity not found"));
        a.backup_type = "podman".into(); state.mode.store(3, Ordering::Relaxed);
        let count = state.calls.load(Ordering::Relaxed);
        assert!(restore(&a, &client(TOKEN)).await.unwrap_err().contains("307"));
        assert_eq!(state.calls.load(Ordering::Relaxed), count + 1);
        assert!(!a.destination.exists()); assert_eq!(fs::read_dir(&t.0).unwrap().count(), 0);
    }
    #[tokio::test]
    async fn corrupt_missing_and_oversized_chunks_leave_unpublished_private_stage() {
        let (_server, state, remote) = start_mock().await;
        for mode in [1, 2, 4] {
            let t = Temp::new(); let a = args(&t, remote.clone()); state.mode.store(mode, Ordering::Relaxed);
            let error = restore(&a, &client(TOKEN)).await.unwrap_err();
            assert!(error.contains("staging retained")); assert!(!a.destination.exists());
            let entries = fs::read_dir(&t.0).unwrap().collect::<Vec<_>>(); assert_eq!(entries.len(), 1);
            assert!(entries[0].as_ref().unwrap().file_name().to_string_lossy().starts_with(".pmbs-restore-"));
        }
    }
    #[tokio::test]
    async fn whole_file_digest_is_checked_independently_of_valid_chunks() {
        let (_server, mut state, _remote) = start_mock().await;
        state.manifest.files.get_mut("nested/file").unwrap().digest = format!("blake3:{}", "0".repeat(64));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let remote = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, Router::new().fallback(get(serve)).with_state(state)).await.unwrap(); });
        let _guard = MockServer(task); let t = Temp::new(); let a = args(&t, remote);
        assert!(restore(&a, &client(TOKEN)).await.unwrap_err().contains("file length/digest mismatch")); assert!(!a.destination.exists());
    }
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    struct EnvGuard(Vec<(&'static str, Option<OsString>)>);
    impl EnvGuard {
        fn capture() -> Self {
            Self(["PMBS_API_TOKEN", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"]
                .into_iter().map(|key| (key, std::env::var_os(key))).collect())
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.0 {
                if let Some(value) = value { std::env::set_var(key, value); }
                else { std::env::remove_var(key); }
            }
        }
    }
    #[tokio::test]
    async fn production_client_requires_token_and_bypasses_proxy_with_real_pmbs_router() {
        // Only this test exercises process token/proxy configuration. All other
        // test clients take explicit tokens and disable proxy lookup entirely.
        let _lock = ENV_LOCK.lock().await;
        let _env = EnvGuard::capture();
        let t = Temp::new();
        let ds = pmbs::Datastore::new(&t.0.join("server"), "store").unwrap();
        let bytes = b"actual PMBS contract".to_vec();
        ds.write_chunk(&bytes).unwrap();
        ds.save_snapshot(&manifest(&[bytes.clone()])).unwrap();
        let state = pmbs::server::AppState {
            base_dir: t.0.join("server"),
            datastores: Arc::new(tokio::sync::RwLock::new(HashMap::from([("store".into(), ds)]))),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let remote = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, pmbs::server::create_router(state)).await.unwrap(); });
        let _server = MockServer(task);
        let a = args(&t, remote);
        std::env::remove_var("PMBS_API_TOKEN");
        assert!(run(&a).await.unwrap_err().contains("required"));
        std::env::set_var("PMBS_API_TOKEN", "short");
        assert!(run(&a).await.is_err());
        assert!(!a.destination.exists());
        let trap = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = format!("http://{}", trap.local_addr().unwrap());
        for key in ["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"] { std::env::set_var(key, &proxy); }
        std::env::set_var("PMBS_API_TOKEN", TOKEN);
        let report = run(&a).await.unwrap();
        assert_eq!(report["metadataSource"], "authenticated PMBS server snapshot GET");
        assert_eq!(fs::read(a.destination.join("nested/file")).unwrap(), bytes);
        assert!(tokio::time::timeout(Duration::from_millis(20), trap.accept()).await.is_err());
    }
    #[test]
    fn non_utf8_destination_refused_before_publication_or_creation() {
        let t = Temp::new();
        use std::os::unix::ffi::OsStringExt;
        let destination = t.0.join(OsString::from_vec(vec![b'o', 0xff]));
        assert!(Staging::new(&destination).is_err());
        assert_eq!(fs::read_dir(&t.0).unwrap().count(), 0);
    }

    #[test]
    fn native_restore_cli_requires_server_snapshot_type_and_destination() {
        use clap::Parser;
        use pmbs::cli::{Cli, Commands};
        let cli = Cli::try_parse_from(["pmbs", "restore", "/owned/new", "--snapshot", "id", "--backup-type", "podman"]).unwrap();
        match cli.command {
            Commands::Restore(a) => { assert_eq!(a.snapshot, "id"); assert_eq!(a.backup_type, "podman"); assert_eq!(a.destination, PathBuf::from("/owned/new")); }
            _ => panic!("restore CLI did not select native restore"),
        }
        assert!(Cli::try_parse_from(["pmbs", "restore", "/owned/new", "--snapshot", "id"]).is_err());
    }

}
