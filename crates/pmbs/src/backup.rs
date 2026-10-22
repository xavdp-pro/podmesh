//! Bounded, operator-quiesced regular-content capture. Linux fd-anchored traversal.
//! Never stops writers, exports source paths/attributes, or promises a live snapshot.
//! Deadline checks are cooperative: a stalled kernel filesystem call is not preempted.
use pmbs::cli::BackupArgs;
use pmbs::datastore::{validate_name, validate_file_path};
use pmbs::server::configured_token;
use pmbs::{BackupFile, BackupManifest, ChunkRef, CHUNK_SIZE};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::ffi::{CString, CStr, OsStr};
use std::fs::{File, Metadata, OpenOptions};
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::{ffi::OsStrExt, fs::{MetadataExt, OpenOptionsExt}};
use std::path::{Path, Component};
use std::time::{Instant, Duration};

const LIST_LIMIT: usize = 32 * 1024 * 1024;
const MANIFEST_LIMIT: usize = 1024 * 1024; // below the unchanged JSON request limit
const ACK_LIMIT: usize = 64 * 1024;
fn err(s: &str) -> String { s.into() }
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp { dev:u64, ino:u64, mode:u32, len:u64, mtime:(i64,i64), ctime:(i64,i64) }
impl Stamp {
    fn of(m: &Metadata) -> Self { Self { dev:m.dev(),ino:m.ino(),mode:m.mode(),len:m.len(),mtime:(m.mtime(),m.mtime_nsec()),ctime:(m.ctime(),m.ctime_nsec()) } }
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry { path:String, stamp:Stamp, directory:bool }
fn component(s: &OsStr) -> Result<CString,String> { CString::new(s.as_bytes()).map_err(|_|err("invalid input component")) }
fn path_at(parent: &File, name: &OsStr) -> Result<File,String> {
    let name=component(name)?;
    // O_PATH opens no device/FIFO: reject type BEFORE any content-readable open.
    let fd=unsafe { libc::openat(parent.as_raw_fd(),name.as_ptr(),libc::O_PATH|libc::O_NOFOLLOW|libc::O_CLOEXEC) };
    if fd<0 { return Err(err("could not anchor input component")); }
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn dir_handle(path: &File) -> Result<File,String> {
    if !path.metadata().map_err(|_|err("input metadata unavailable"))?.is_dir() { return Err(err("input ancestor is not a real directory")); }
    let dot=CString::new(".").unwrap();
    let fd=unsafe { libc::openat(path.as_raw_fd(),dot.as_ptr(),libc::O_RDONLY|libc::O_DIRECTORY|libc::O_NOFOLLOW|libc::O_CLOEXEC) };
    if fd<0 { return Err(err("could not read anchored directory")); }
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn open_root(source: &Path) -> Result<File,String> {
    if source.components().any(|c| matches!(c,Component::ParentDir)) { return Err(err("source parent traversal refused")); }
    let full=if source.is_absolute() { source.to_owned() } else { std::env::current_dir().map_err(|_|err("working directory unavailable"))?.join(source) };
    if full.to_str().is_none() { return Err(err("source path must be UTF8")); }
    let mut dir=File::open("/").map_err(|_|err("input root unavailable"))?;
    for c in full.components() { if let Component::Normal(name)=c { dir=dir_handle(&path_at(&dir,name)?)?; } }
    Ok(dir)
}
struct DirStream(*mut libc::DIR);
impl Drop for DirStream { fn drop(&mut self) { unsafe { libc::closedir(self.0); } } }
struct Budget { end:Instant, max_files:usize, max_bytes:u64, max_file:u64, entries:usize, paths:usize, files:usize, bytes:u64 }
impl Budget {
    fn new(a:&BackupArgs) -> Result<Self,String> {
        if !a.quiesced_input || a.max_files==0 || a.max_files>10000 || a.max_bytes==0 || a.max_file_bytes==0 || a.max_seconds==0 { return Err(err("quiesced acknowledgement and nonzero finite limits required (max-files <= 10000)")); }
        Ok(Self { end:Instant::now().checked_add(Duration::from_secs(a.max_seconds)).ok_or_else(||err("invalid capture deadline"))?,max_files:a.max_files,max_bytes:a.max_bytes,max_file:a.max_file_bytes.min(a.max_bytes),entries:0,paths:0,files:0,bytes:0 })
    }
    fn check(&self) -> Result<(),String> { if Instant::now()>=self.end { Err(err("capture deadline exceeded")) } else { Ok(()) } }
    fn timeout(&self) -> Result<Duration,String> { self.check()?; Ok(self.end.saturating_duration_since(Instant::now()).min(Duration::from_secs(30))) }
}
fn scan(dir:&File,prefix:&str,depth:usize,b:&mut Budget,out:&mut Vec<Entry>) -> Result<(),String> {
    b.check()?; if depth>64 { return Err(err("input directory depth limit exceeded")); }
    // openat(".") gives a NEW description; dup only hands that new offset to fdopendir.
    let iterable=dir_handle(dir)?;
    let raw=unsafe { libc::fcntl(iterable.as_raw_fd(),libc::F_DUPFD_CLOEXEC,0) };
    if raw<0 { return Err(err("directory stream unavailable")); }
    let ptr=unsafe { libc::fdopendir(raw) };
    if ptr.is_null() { unsafe { libc::close(raw); } return Err(err("directory stream unavailable")); }
    let stream=DirStream(ptr);
    loop {
        b.check()?;
        unsafe { *libc::__errno_location()=0; }
        let de=unsafe { libc::readdir(stream.0) };
        if de.is_null() { if unsafe { *libc::__errno_location() }!=0 { return Err(err("input enumeration failed")); } break; }
        let name=unsafe { CStr::from_ptr((*de).d_name.as_ptr()) }.to_str().map_err(|_|err("input filenames must be UTF8"))?;
        if name=="." || name==".." { continue; }
        let path=if prefix.is_empty() { name.to_owned() } else { format!("{prefix}/{name}") };
        validate_file_path(&path).map_err(|_|err("invalid relative input filename"))?;
        b.entries+=1;b.paths=b.paths.checked_add(path.len()).ok_or_else(||err("input path budget exceeded"))?;
        if b.entries>b.max_files*2+1024 || b.paths>4*1024*1024 { return Err(err("input entry/path budget exceeded")); }
        let anchored=path_at(dir,OsStr::new(name))?;let m=anchored.metadata().map_err(|_|err("input metadata unavailable"))?;
        if !m.is_dir() && !m.is_file() { return Err(err("nonregular input refused (including symlinks/special files)")); }
        if m.is_file() {
            b.files+=1;b.bytes=b.bytes.checked_add(m.len()).ok_or_else(||err("input size overflow"))?;
            if b.files>b.max_files || m.len()>b.max_file || b.bytes>b.max_bytes { return Err(err("input exceeds declared file/byte limits")); }
        }
        out.push(Entry { path:path.clone(),stamp:Stamp::of(&m),directory:m.is_dir() });
        if m.is_dir() { scan(&dir_handle(&anchored)?,&path,depth+1,b,out)?; }
    }
    Ok(())
}
fn inventory(root:&File,a:&BackupArgs,end:Instant) -> Result<Vec<Entry>,String> {
    let mut b=Budget::new(a)?;b.end=end;let mut entries=Vec::new();scan(root,"",0,&mut b,&mut entries)?;
    entries.sort_by(|a,b|a.path.cmp(&b.path));Ok(entries)
}
fn open_input(root:&File,e:&Entry) -> Result<File,String> {
    let mut parts=e.path.split('/').collect::<Vec<_>>();let last=parts.pop().ok_or_else(||err("invalid input path"))?;
    let mut dir=dir_handle(root)?;
    for name in parts { dir=dir_handle(&path_at(&dir,OsStr::new(name))?)?; }
    let path=path_at(&dir,OsStr::new(last))?;let m=path.metadata().map_err(|_|err("input metadata unavailable"))?;
    if !m.is_file() || Stamp::of(&m)!=e.stamp { return Err(err("input file incarnation changed")); }
    // This procfs link is to OUR held, type-checked descriptor, never a user link.
    let file=OpenOptions::new().read(true).custom_flags(libc::O_CLOEXEC|libc::O_NONBLOCK)
        .open(format!("/proc/self/fd/{}",path.as_raw_fd())).map_err(|_|err("could not read anchored regular input"))?;
    if Stamp::of(&file.metadata().map_err(|_|err("input metadata unavailable"))?)!=e.stamp { return Err(err("input file incarnation changed")); }
    Ok(file)
}
fn endpoint(remote:&str) -> Result<reqwest::Url,String> {
    let u=reqwest::Url::parse(remote).map_err(|_|err("invalid backup endpoint"))?;
    let local=u.host_str().and_then(|h|h.trim_start_matches('[').trim_end_matches(']').parse::<std::net::IpAddr>().ok()).map(|ip|ip.is_loopback()).unwrap_or(false);
    if !u.username().is_empty() || u.password().is_some() || u.path()!="/" || u.query().is_some() || u.fragment().is_some() || !(u.scheme()=="https" || u.scheme()=="http" && local) { return Err(err("endpoint requires HTTPS or numeric-loopback HTTP without credentials/path/query/fragment")); }
    Ok(u)
}
fn client(token:&str) -> Result<reqwest::Client,String> {
    let mut h=reqwest::header::HeaderMap::new();let mut v=reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_|err("invalid token header"))?;v.set_sensitive(true);h.insert(reqwest::header::AUTHORIZATION,v);
    reqwest::Client::builder().default_headers(h).no_proxy().redirect(reqwest::redirect::Policy::none()).build().map_err(|_|err("backup client unavailable"))
}
async fn response(mut r:reqwest::Response,limit:usize) -> Result<Vec<u8>,String> {
    if !r.status().is_success() { return Err(format!("PMBS request refused (HTTP {})",r.status().as_u16())); }
    if r.content_length().map(|n|n>limit as u64).unwrap_or(false) { return Err(err("PMBS response exceeds limit")); }
    let mut out=Vec::new();while let Some(b)=r.chunk().await.map_err(|_|err("PMBS response read failed"))? { if b.len()>limit.saturating_sub(out.len()) { return Err(err("PMBS response exceeds limit")); }out.extend_from_slice(&b); }Ok(out)
}
#[derive(Deserialize)]
struct Listing { ok:bool, snapshots:Vec<BackupManifest> }
pub async fn run(a:&BackupArgs) -> Result<Value,String> {
    let token=configured_token()?;capture(a,&client(&token)?).await
}
async fn capture(a:&BackupArgs,c:&reqwest::Client) -> Result<Value,String> {
    for n in [&a.snapshot,&a.backup_type,&a.datastore] { validate_name(n).map_err(|_|err("invalid snapshot/datastore component"))?; }
    let b=Budget::new(a)?;let base=endpoint(&a.remote)?;let root=open_root(&a.source)?;let root_stamp=Stamp::of(&root.metadata().map_err(|_|err("source metadata unavailable"))?);
    let entries=inventory(&root,a,b.end)?;
    let ds=format!("api/v1/datastores/{}/",a.datastore);
    let r=c.get(base.join(&(ds.clone()+"snapshots")).map_err(|_|err("invalid request path"))?).timeout(b.timeout()?).send().await.map_err(|_|err("PMBS listing request failed"))?;
    let listing:Listing=serde_json::from_slice(&response(r,LIST_LIMIT).await?).map_err(|_|err("invalid PMBS listing"))?;
    if !listing.ok || listing.snapshots.iter().any(|s|s.id==a.snapshot && s.backup_type==a.backup_type) { return Err(err("snapshot identity already exists or listing failed")); }
    let mut manifest=BackupManifest { id:a.snapshot.clone(),backup_type:a.backup_type.clone(),backup_time:chrono::Utc::now().to_rfc3339(),files:HashMap::new(),total_logical_bytes:0,chunks_count:0,verified:false,verified_at:None };
    let mut metadata_budget=512usize;
    for e in entries.iter().filter(|e|!e.directory) {
        b.check()?;let mut input=open_input(&root,e)?;let mut offset=0u64;let mut hash=Sha256::new();let mut chunks=Vec::new();
        metadata_budget=metadata_budget.checked_add(e.path.len()*2+512).ok_or_else(||err("manifest budget exceeded"))?;
        if metadata_budget>MANIFEST_LIMIT { return Err(err("manifest budget exceeded")); }
        while offset<e.stamp.len {
            b.check()?;let size=(e.stamp.len-offset).min(CHUNK_SIZE as u64) as usize;let mut data=vec![0;size];
            input.read_exact(&mut data).map_err(|_|err("input changed/truncated or read failed"))?;b.check()?;
            let id=format!("sha256:{}",hex::encode(Sha256::digest(&data)));hash.update(&data);
            metadata_budget=metadata_budget.checked_add(192).ok_or_else(||err("manifest budget exceeded"))?;if metadata_budget>MANIFEST_LIMIT { return Err(err("manifest budget exceeded")); }
            let r=c.post(base.join(&(ds.clone()+"chunks")).map_err(|_|err("invalid request path"))?).timeout(b.timeout()?).header(reqwest::header::CONTENT_TYPE,"application/octet-stream").body(data).send().await.map_err(|_|err("PMBS chunk upload failed"))?;
            let ack:Value=serde_json::from_slice(&response(r,ACK_LIMIT).await?).map_err(|_|err("invalid chunk acknowledgement"))?;
            if ack["ok"]!=true || ack["chunkId"]!=id { return Err(err("chunk acknowledgement identity mismatch")); }
            chunks.push(ChunkRef { id,size,offset });offset+=size as u64;
        }
        let mut extra=[0];if input.read(&mut extra).map_err(|_|err("input read failed"))?!=0 || Stamp::of(&input.metadata().map_err(|_|err("input metadata unavailable"))?)!=e.stamp { return Err(err("input changed during capture")); }b.check()?;
        manifest.total_logical_bytes+=offset;manifest.chunks_count+=chunks.len();
        manifest.files.insert(e.path.clone(),BackupFile { path:e.path.clone(),size:offset,digest:format!("sha256:{}",hex::encode(hash.finalize())),chunks_count:chunks.len(),chunks });
    }
    if inventory(&root,a,b.end)?!=entries || Stamp::of(&root.metadata().map_err(|_|err("source metadata unavailable"))?)!=root_stamp || Stamp::of(&open_root(&a.source)?.metadata().map_err(|_|err("source metadata unavailable"))?)!=root_stamp { return Err(err("source tree changed during capture")); }
    let body=serde_json::to_vec(&manifest).map_err(|_|err("manifest encoding failed"))?;if body.len()>MANIFEST_LIMIT { return Err(err("manifest size limit exceeded")); }
    let r=c.post(base.join(&(ds+"snapshots")).map_err(|_|err("invalid request path"))?).timeout(b.timeout()?).header(reqwest::header::IF_NONE_MATCH,"*").header(reqwest::header::CONTENT_TYPE,"application/json").body(body).send().await.map_err(|_|err("snapshot publication not confirmed; identity may exist, do not blindly retry"))?;
    let ack:Value=serde_json::from_slice(&response(r,ACK_LIMIT).await.map_err(|_|err("snapshot publication not confirmed; identity may exist"))?).map_err(|_|err("invalid publication acknowledgement; identity may exist"))?;
    if ack["ok"]!=true || ack["snapshotId"]!=a.snapshot || ack["createdOnly"]!=true { return Err(err("create-only publication not confirmed; identity may exist")); }
    let mut files=manifest.files.values().map(|f|json!({"path":f.path,"bytes":f.size,"digest":f.digest})).collect::<Vec<_>>();
    files.sort_by(|a,b|a["path"].as_str().cmp(&b["path"].as_str()));
    Ok(json!({"ok":true,"snapshot":a.snapshot,"backupType":a.backup_type,"datastore":a.datastore,"bytes":manifest.total_logical_bytes,"fileCount":files.len(),"files":files,"chunks":manifest.chunks_count,"publication":"created-only","quiescedInputAcknowledged":true,"bootReadiness":"NOT_TESTED","scope":"regular file contents only; no attributes/links/workload stop/start/live snapshot guarantee"}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, sync::{Arc, atomic::{AtomicUsize,Ordering}}};
    use std::os::unix::fs::{DirBuilderExt,symlink};
    use axum::{Router,extract::State,body::Bytes,response::{IntoResponse,Response},http::{StatusCode,Uri}};
    struct Temp(std::path::PathBuf);
    impl Temp {
        fn new()->Self {
            let mut random=[0;16];File::open("/dev/urandom").unwrap().read_exact(&mut random).unwrap();
            let p=std::env::temp_dir().canonicalize().unwrap().join(format!("pmbs-backup-{}",hex::encode(random)));
            fs::DirBuilder::new().mode(0o700).create(&p).unwrap();Self(p)
        }
    }
    impl Drop for Temp { fn drop(&mut self) { let _=fs::remove_dir_all(&self.0); } }
    const TOKEN:&str="backup-test-only-012345678901234567890";
    fn args(t:&Temp,remote:String)->BackupArgs {
        BackupArgs {source:t.0.join("input"),quiesced_input:true,snapshot:"backup".into(),backup_type:"regular".into(),datastore:"store".into(),remote,max_bytes:8*CHUNK_SIZE as u64,max_files:16,max_file_bytes:8*CHUNK_SIZE as u64,max_seconds:30}
    }
    #[test]
    fn cli_requires_quiescence_and_explicit_limits() {
        use clap::Parser;
        use pmbs::cli::{Cli,Commands};
        let ok=["pmbs","backup","input","--quiesced-input","--snapshot","s","--backup-type","regular","--max-bytes","100","--max-files","2"];
        assert!(matches!(Cli::try_parse_from(ok).unwrap().command,Commands::Backup(_)));
        assert!(Cli::try_parse_from(ok.into_iter().filter(|s|*s!="--quiesced-input")).is_err());
        let t=Temp::new();let mut a=args(&t,"http://127.0.0.1:1".into());a.max_files=0;assert!(Budget::new(&a).is_err());a.max_files=1;a.quiesced_input=false;assert!(Budget::new(&a).is_err());
        for u in ["http://example.com","http://localhost:8007","https://user:secret@example.com","https://example.com/path"] { assert!(endpoint(u).is_err()); }
    }
    #[test]
    fn anchored_inventory_refuses_links_special_non_utf8_and_limits() {
        let t=Temp::new();let a=args(&t,"http://127.0.0.1:1".into());fs::create_dir(&a.source).unwrap();fs::write(a.source.join("regular"),b"data").unwrap();
        let root=open_root(&a.source).unwrap();let end=Budget::new(&a).unwrap().end;
        let e=inventory(&root,&a,end).unwrap();assert_eq!(e.len(),1);
        symlink(t.0.join("outside"),a.source.join("link")).unwrap();assert!(inventory(&root,&a,end).is_err());fs::remove_file(a.source.join("link")).unwrap();
        let fifo=a.source.join("fifo");let c=CString::new(fifo.as_os_str().as_bytes()).unwrap();assert_eq!(unsafe {libc::mkfifo(c.as_ptr(),0o600)},0);assert!(inventory(&root,&a,end).is_err());fs::remove_file(fifo).unwrap();
        use std::os::unix::ffi::OsStringExt;
        let invalid=a.source.join(std::ffi::OsString::from_vec(vec![0xff]));
        match fs::write(&invalid,b"data") {
            Ok(()) => { assert!(inventory(&root,&a,end).is_err());fs::remove_file(invalid).unwrap(); }
            Err(e) if matches!(e.raw_os_error(),Some(libc::EILSEQ)|Some(libc::EINVAL)) => eprintln!("test filesystem refuses non-UTF8 fixture; native non-UTF8 branch not exercised"),
            Err(e) => panic!("could not create non-UTF8 test fixture: {e}"),
        }
        for name in ["bad:name", "bad\\name", "bad\nname"] {
            fs::write(a.source.join(name),b"data").unwrap();assert!(inventory(&root,&a,end).is_err());fs::remove_file(a.source.join(name)).unwrap();
        }
        let mut small=args(&t,a.remote.clone());small.max_bytes=1;assert!(inventory(&root,&small,end).is_err());
        fs::write(a.source.join("extra"),b"x").unwrap();let mut few=args(&t,a.remote.clone());few.max_files=1;assert!(inventory(&root,&few,end).is_err());fs::remove_file(a.source.join("extra")).unwrap();
        assert!(inventory(&root,&a,Instant::now()-Duration::from_secs(1)).is_err());
        symlink(&a.source,t.0.join("alias")).unwrap();assert!(open_root(&t.0.join("alias")).is_err());
        fs::write(a.source.join("regular"),b"changed").unwrap();assert!(open_input(&root,&e[0]).is_err());
    }
    #[test]
    fn real_router_roundtrip_and_atomic_conditional_publication() {
        // Isolate process environment from restore's pre-existing env tests even
        // when all binary tests run in parallel. The child runs THIS exact test.
        if std::env::var_os("PMBS_BACKUP_TEST_CHILD").is_none() {
            let out=std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact","backup::tests::real_router_roundtrip_and_atomic_conditional_publication","--nocapture"])
                .env("PMBS_BACKUP_TEST_CHILD","1").env("PMBS_API_TOKEN",TOKEN).output().unwrap();
            let stdout=String::from_utf8_lossy(&out.stdout);
            assert!(out.status.success() && stdout.contains("test result: ok. 1 passed;"),"{}\n{}",stdout,String::from_utf8_lossy(&out.stderr));return;
        }
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let t=Temp::new();let mut a=args(&t,String::new());fs::create_dir(&a.source).unwrap();
            let bytes=vec![7;CHUNK_SIZE+17];fs::write(a.source.join("multi"),&bytes).unwrap();fs::write(a.source.join("empty"),[]).unwrap();
            let ds=pmbs::Datastore::new(&t.0.join("server"),"store").unwrap();
            let state=pmbs::server::AppState {base_dir:t.0.join("server"),datastores:Arc::new(tokio::sync::RwLock::new(HashMap::from([("store".into(),ds.clone())])))};
            let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();a.remote=format!("http://{}",listener.local_addr().unwrap());
            let task=tokio::spawn(async move {axum::serve(listener,pmbs::server::create_router(state)).await.unwrap()});
            let c=client(TOKEN).unwrap();let url=format!("{}/api/v1/datastores/store/snapshots",a.remote);
            assert_eq!(client("wrong").unwrap().get(&url).send().await.unwrap().status(),StatusCode::UNAUTHORIZED);
            let result=capture(&a,&c).await.unwrap();assert_eq!(result["bytes"],bytes.len());assert_eq!(result["publication"],"created-only");
            let original=ds.list_snapshots().pop().unwrap();assert_eq!(original.files["multi"].chunks.len(),2);assert!(original.files["empty"].chunks.is_empty());
            let text=serde_json::to_string(&original).unwrap();assert!(!text.contains(a.source.to_str().unwrap()));assert!(!text.contains(TOKEN));
            assert!(capture(&a,&c).await.is_err());
            let original_path=ds.snapshots_dir.join(&original.backup_type).join(&original.id).join("manifest.json");
            let before=fs::read(&original_path).unwrap();
            assert_eq!(c.post(&url).header("If-None-Match","*").json(&original).send().await.unwrap().status(),StatusCode::PRECONDITION_FAILED);
            let mut different=original.clone();different.backup_time="2026-10-21T00:00:00Z".into();
            assert_eq!(c.post(&url).header("If-None-Match","*").json(&different).send().await.unwrap().status(),StatusCode::PRECONDITION_FAILED);
            assert_eq!(c.post(&url).header("If-None-Match","*").header("If-None-Match","other").json(&original).send().await.unwrap().status(),StatusCode::BAD_REQUEST);
            assert_eq!(fs::read(&original_path).unwrap(),before);
            assert_eq!(c.post(&url).json(&original).send().await.unwrap().status(),StatusCode::CREATED); // old retry contract
            let mut race=original.clone();race.id="race".into();
            let (x,y)=tokio::join!(c.post(&url).header("If-None-Match","*").json(&race).send(),c.post(&url).header("If-None-Match","*").json(&race).send());
            let mut codes=vec![x.unwrap().status().as_u16(),y.unwrap().status().as_u16()];codes.sort();assert_eq!(codes,vec![201,412]);
            let barrier=Arc::new(std::sync::Barrier::new(2));let mut threads=Vec::new();
            for _ in 0..2 {
                let barrier=barrier.clone();let ds=ds.clone();let mut m=original.clone();m.id="thread-race".into();
                threads.push(std::thread::spawn(move || {barrier.wait();ds.create_snapshot(&m).map_err(|e|e.kind())}));
            }
            let outcomes=threads.into_iter().map(|t|t.join().unwrap()).collect::<Vec<_>>();
            assert_eq!(outcomes.iter().filter(|r|r.is_ok()).count(),1);
            assert_eq!(outcomes.iter().filter(|r|matches!(r,Err(std::io::ErrorKind::AlreadyExists))).count(),1);
            let mut missing=original.clone();missing.id="missing".into();missing.files.get_mut("multi").unwrap().chunks[0].id=format!("sha256:{}","0".repeat(64));
            assert_eq!(c.post(&url).header("If-None-Match","*").json(&missing).send().await.unwrap().status(),StatusCode::BAD_REQUEST);
            let restore=pmbs::cli::RestoreArgs {destination:t.0.join("restored"),datastore:"store".into(),snapshot:a.snapshot.clone(),backup_type:a.backup_type.clone(),remote:a.remote.clone(),max_bytes:a.max_bytes};
            crate::restore::run(&restore).await.unwrap();assert_eq!(fs::read(restore.destination.join("multi")).unwrap(),bytes);assert_eq!(fs::metadata(restore.destination.join("empty")).unwrap().len(),0);
            task.abort();
        });
    }
    #[derive(Clone)]
    struct Fault {mode:usize,input:std::path::PathBuf,published:Arc<AtomicUsize>}
    async fn fault(State(s):State<Fault>,uri:Uri,body:Bytes)->Response {
        if uri.path().ends_with("snapshots") && body.is_empty() {return axum::Json(json!({"ok":true,"snapshots":[]})).into_response()}
        if uri.path().ends_with("chunks") {
            if s.mode==1 {return StatusCode::INTERNAL_SERVER_ERROR.into_response()}
            if s.mode==2 {fs::write(&s.input,b"mutated and grew").unwrap();}
            if s.mode==3 {fs::write(&s.input,[]).unwrap();}
            let id=if s.mode==0 {format!("sha256:{}","0".repeat(64))} else {format!("sha256:{}",hex::encode(Sha256::digest(&body)))};
            return axum::Json(json!({"ok":true,"chunkId":id})).into_response();
        }
        s.published.fetch_add(1,Ordering::Relaxed);StatusCode::INTERNAL_SERVER_ERROR.into_response()
    }
    #[tokio::test]
    async fn corrupt_ack_http_failure_and_source_growth_truncation_never_publish() {
        for mode in [0,1,2,3] {
            let t=Temp::new();let mut a=args(&t,String::new());fs::create_dir(&a.source).unwrap();fs::write(a.source.join("data"),b"data").unwrap();
            let published=Arc::new(AtomicUsize::new(0));let s=Fault {mode,input:a.source.join("data"),published:published.clone()};
            let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();a.remote=format!("http://{}",listener.local_addr().unwrap());
            let task=tokio::spawn(async move {axum::serve(listener,Router::new().fallback(fault).with_state(s)).await.unwrap()});
            let error=capture(&a,&client(TOKEN).unwrap()).await.unwrap_err();assert!(!error.contains(t.0.to_str().unwrap()));assert_eq!(published.load(Ordering::Relaxed),0);task.abort();
        }
    }
}
