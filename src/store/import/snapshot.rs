//! Read original bundle only through checked OS descriptors. SQLite opens private copies only.
use super::{canonical, caps::Caps, refusal};
use crate::store::Result;
use rusqlite::{backup::{Backup, StepResult}, Connection, OpenFlags};
use serde_json::{json, Value as Json};
use std::{fs::{self, File, OpenOptions}, io::{Read, Seek, SeekFrom, Write}, path::{Path, PathBuf}};
use std::os::unix::{fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt}};

fn io_error(_: std::io::Error) -> crate::store::StoreError { refusal("snapshot_io_failed") }
fn sqlite_error(_: rusqlite::Error) -> crate::store::StoreError { refusal("private_snapshot_sqlite_failed") }

pub fn create_file(path: &Path) -> Result<File> {
    OpenOptions::new().write(true).create_new(true).mode(0o600).open(path).map_err(io_error)
}

pub fn private_json(path: &Path, value: &Json) -> Result<()> {
    let mut file = create_file(path)?;
    serde_json::to_writer_pretty(&mut file, value).map_err(|_| refusal("receipt_write_failed"))?;
    file.write_all(b"\n").map_err(io_error)?;
    file.sync_all().map_err(io_error)?;
    sync_directory(path.parent().ok_or_else(|| refusal("invalid_receipt_parent"))?)
}

pub fn sync_directory(path: &Path) -> Result<()> { File::open(path).map_err(io_error)?.sync_all().map_err(io_error) }

fn read_regular(path: &Path) -> Result<File> {
    // Linux O_NOFOLLOW. PodMesh's supported host/snapshot path is Linux.
    let file = OpenOptions::new().read(true).custom_flags(0o400000).open(path).map_err(io_error)?;
    let fd = file.metadata().map_err(io_error)?;
    let named = fs::symlink_metadata(path).map_err(io_error)?;
    if !fd.is_file() || !named.is_file() || fd.dev() != named.dev() || fd.ino() != named.ino() {
        return Err(refusal("source_file_identity_mismatch"));
    }
    Ok(file)
}

fn fingerprint(file: &mut File) -> Result<Json> {
    use sha2::{Digest, Sha256};
    let before = file.metadata().map_err(io_error)?;
    file.seek(SeekFrom::Start(0)).map_err(io_error)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 65536];
    let mut count = 0_u64;
    loop {
        let n = file.read(&mut buffer).map_err(io_error)?;
        if n == 0 { break; }
        count = count.checked_add(n as u64).ok_or_else(|| refusal("resource_counter_overflow"))?;
        if count > before.len() { return Err(refusal("source_file_drift")); }
        hash.update(&buffer[..n]);
    }
    let after = file.metadata().map_err(io_error)?;
    if count != before.len() || before.len() != after.len() || before.mtime() != after.mtime() || before.mtime_nsec() != after.mtime_nsec() {
        return Err(refusal("source_file_drift"));
    }
    Ok(json!({"dev":before.dev(),"inode":before.ino(),"bytes":count,"mode":before.mode(),"mtime":before.mtime(),"mtime_nsec":before.mtime_nsec(),"sha256":format!("{:x}",hash.finalize())}))
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string(); name.push(suffix); PathBuf::from(name)
}

fn bundle_files(path: &Path, caps: &Caps) -> Result<Vec<(String, Option<File>)>> {
    if fs::symlink_metadata(sidecar(path,"-journal")).is_ok() { return Err(refusal("rollback_journal_source_refused")); }
    let mut result = Vec::new(); let mut total = 0_u64;
    for (role,suffix) in [("main",""),("wal","-wal"),("shm","-shm")] {
        let named = sidecar(path,suffix);
        let file = match fs::symlink_metadata(&named) {
            Ok(_) => Some(read_regular(&named)?),
            Err(e) if e.kind()==std::io::ErrorKind::NotFound && role != "main" => None,
            Err(_) => return Err(refusal("source_bundle_missing")),
        };
        if let Some(file) = &file { total=total.checked_add(file.metadata().map_err(io_error)?.len()).ok_or_else(|| refusal("resource_counter_overflow"))?; }
        if total > caps.bundle { return Err(refusal("source_limit_exceeded")); }
        result.push((role.to_owned(),file));
    }
    if result[1].1.is_some() != result[2].1.is_some() { return Err(refusal("incomplete_wal_shm_bundle")); }
    Ok(result)
}

pub fn bundle_manifest(path: &Path, caps: &Caps) -> Result<Json> {
    let mut result=serde_json::Map::new();
    for (role,mut file) in bundle_files(path,caps)? {
        result.insert(role,match &mut file { Some(file)=>fingerprint(file)?,None=>Json::Null });
    }
    Ok(Json::Object(result))
}

pub struct Snapshot {
    pub path: PathBuf,
    pub seal: Json,
}

pub fn capture(source: &Path, manifest: &Json, destination: &Path, caps: &Caps) -> Result<Snapshot> {
    capture_observed(source,manifest,destination,caps,&mut |_|Ok(()))
}

/// Campaign fault observer operates only on private clone/seal boundaries. The original
/// remains descriptor-only; errors and external SIGKILL never trigger original cleanup.
pub fn capture_observed(source:&Path,manifest:&Json,destination:&Path,caps:&Caps,event:&mut dyn FnMut(&str)->Result<()>)->Result<Snapshot> {
    if manifest["synthetic_only"] != true || manifest["customer_data"] != false || manifest["real_keys_or_credentials"] != false
        || manifest["all_owners_and_maintenance_quiesced"] != true || manifest["generator"] != "podmesh-c02-synthetic/1"
        || manifest["source_path"].as_str() != source.to_str() {
        return Err(refusal("synthetic_source_contract_required"));
    }
    let lock_path=manifest["source_lock"].as_str().ok_or_else(|| refusal("source_lease_missing"))?;
    let lease=read_regular(Path::new(lock_path))?;
    lease.lock_shared().map_err(io_error)?;
    let mut files=bundle_files(source,caps)?;
    let mut before=serde_json::Map::new();
    for (role,file) in &mut files { before.insert(role.clone(),match file {Some(file)=>fingerprint(file)?,None=>Json::Null}); }
    if Json::Object(before.clone()) != manifest["bundle"] { return Err(refusal("source_bundle_manifest_mismatch")); }
    fs::DirBuilder::new().mode(0o700).create(destination).map_err(io_error)?;
    fs::set_permissions(destination,fs::Permissions::from_mode(0o700)).map_err(io_error)?;
    let clone_path=destination.join("clone.sqlite");
    for (role,file) in &mut files {
        if let Some(file)=file {
            let target=match role.as_str(){"main"=>clone_path.clone(),"wal"=>sidecar(&clone_path,"-wal"),"shm"=>sidecar(&clone_path,"-shm"),_=>unreachable!()};
            let mut out=create_file(&target)?;
            file.seek(SeekFrom::Start(0)).map_err(io_error)?;
            let expected=before[role.as_str()]["bytes"].as_u64().ok_or_else(|| refusal("source_bundle_manifest_mismatch"))?;
            let copied=std::io::copy(&mut file.take(expected+1),&mut out).map_err(io_error)?;
            if copied!=expected { return Err(refusal("source_file_drift")); }
            out.sync_all().map_err(io_error)?;
        }
    }
    sync_directory(destination)?;
    if bundle_manifest(source,caps)? != Json::Object(before.clone()) { return Err(refusal("source_file_drift")); }
    event("after_source_bundle_clone")?;
    // First SQLite open: never the original path or its original companion files.
    let clone=Connection::open_with_flags(&clone_path,OpenFlags::SQLITE_OPEN_READ_WRITE).map_err(sqlite_error)?;
    clone.execute_batch("BEGIN;").map_err(sqlite_error)?;
    let integrity:String=clone.query_row("PRAGMA integrity_check",[],|r|r.get(0)).map_err(sqlite_error)?;
    if integrity!="ok" { return Err(refusal("private_clone_integrity_failed")); }
    let snapshot_path=destination.join("snapshot.sqlite");
    create_file(&snapshot_path)?.sync_all().map_err(io_error)?;
    let mut snapshot=Connection::open_with_flags(&snapshot_path,OpenFlags::SQLITE_OPEN_READ_WRITE).map_err(sqlite_error)?;
    {
        let backup=Backup::new(&clone,&mut snapshot).map_err(sqlite_error)?;
        loop {
            match backup.step(128).map_err(sqlite_error)? {
                StepResult::Done=>break,
                StepResult::More=>{},
                _=>return Err(refusal("private_backup_not_completed")),
            }
            if fs::metadata(&snapshot_path).map_err(io_error)?.len()>caps.snapshot { return Err(refusal("source_limit_exceeded")); }
        }
    }
    let integrity:String=snapshot.query_row("PRAGMA integrity_check",[],|r|r.get(0)).map_err(sqlite_error)?;
    if integrity!="ok" { return Err(refusal("private_snapshot_integrity_failed")); }
    drop(snapshot);drop(clone);
    let mut file=read_regular(&snapshot_path)?;
    let record=fingerprint(&mut file)?;
    if record["bytes"].as_u64().unwrap_or(u64::MAX)>caps.snapshot { return Err(refusal("source_limit_exceeded")); }
    file.sync_all().map_err(io_error)?;
    if bundle_manifest(source,caps)? != Json::Object(before.clone()) { return Err(refusal("source_file_drift")); }
    fs::set_permissions(&snapshot_path,fs::Permissions::from_mode(0o400)).map_err(io_error)?;
    let seal=json!({"format":"podmesh-c02-private-snapshot/1","snapshot_sha256":record["sha256"],"snapshot_bytes":record["bytes"],"source_bundle_before":before,"source_bundle_after":manifest["bundle"],"source_manifest_sha256":canonical::hash(serde_json::to_vec(manifest).unwrap().as_slice()),"all_owners_quiesced":true});
    private_json(&destination.join("seal.json"),&seal)?;
    event("after_snapshot_seal")?;
    drop(lease);
    Ok(Snapshot{path:snapshot_path,seal})
}

/// The connection reads a new private immutable object copied from the verified descriptor.
/// Source path replacement or added sidecars cannot redirect SQLite's actual input object.
pub struct SealedReader {
    db: Connection,
    pub identity: Json,
    pub read_path: PathBuf,
}

impl std::ops::Deref for SealedReader {
    type Target = Connection;
    fn deref(&self) -> &Connection { &self.db }
}

fn reject_sidecars(path: &Path) -> Result<()> {
    for suffix in ["-wal","-shm","-journal"] {
        match fs::symlink_metadata(sidecar(path,suffix)) {
            Ok(_) => return Err(refusal("sealed_snapshot_sidecar_present")),
            Err(e) if e.kind()==std::io::ErrorKind::NotFound => {},
            Err(_) => return Err(refusal("sealed_snapshot_sidecar_check_failed")),
        }
    }
    Ok(())
}

pub fn open_sealed(path: &Path, seal: &Json) -> Result<SealedReader> {
    open_sealed_checked(path,seal,||{})
}

fn open_sealed_checked(path: &Path, seal: &Json, after_verify: impl FnOnce()) -> Result<SealedReader> {
    use std::sync::atomic::{AtomicU64,Ordering};
    static NEXT:AtomicU64=AtomicU64::new(0);
    if seal["format"] != "podmesh-c02-private-snapshot/1" || seal["snapshot_bytes"].as_u64().filter(|n| *n <= 33_554_432).is_none() { return Err(refusal("sealed_snapshot_mismatch")); }
    reject_sidecars(path)?;
    let mut file=read_regular(path)?;
    let size=seal["snapshot_bytes"].as_u64().ok_or_else(||refusal("sealed_snapshot_mismatch"))?;
    if file.metadata().map_err(io_error)?.len()!=size {return Err(refusal("sealed_snapshot_mismatch"));}
    let actual=fingerprint(&mut file)?;
    if actual["sha256"]!=seal["snapshot_sha256"] || actual["bytes"]!=seal["snapshot_bytes"] { return Err(refusal("sealed_snapshot_mismatch")); }
    after_verify();
    reject_sidecars(path)?;
    let named=fs::symlink_metadata(path).map_err(io_error)?;
    if !named.is_file() || named.dev()!=actual["dev"].as_u64().unwrap_or(u64::MAX) || named.ino()!=actual["inode"].as_u64().unwrap_or(u64::MAX) {return Err(refusal("sealed_snapshot_identity_changed"));}
    let directory=path.parent().ok_or_else(||refusal("invalid_snapshot_parent"))?.join(format!("reader-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
    fs::DirBuilder::new().mode(0o700).create(&directory).map_err(io_error)?;
    let read_path=directory.join("read.sqlite");
    let mut out=create_file(&read_path)?;
    file.seek(SeekFrom::Start(0)).map_err(io_error)?;
    if std::io::copy(&mut (&mut file).take(size+1),&mut out).map_err(io_error)?!=size {return Err(refusal("sealed_snapshot_mismatch"));}
    out.sync_all().map_err(io_error)?;
    fs::set_permissions(&read_path,fs::Permissions::from_mode(0o400)).map_err(io_error)?;
    let identity=fingerprint(&mut read_regular(&read_path)?)?;
    if identity["sha256"]!=seal["snapshot_sha256"] || fingerprint(&mut file)?!=actual {return Err(refusal("sealed_snapshot_mismatch"));}
    reject_sidecars(path)?;
    let named=fs::symlink_metadata(path).map_err(io_error)?;
    if !named.is_file() || named.dev()!=actual["dev"].as_u64().unwrap_or(u64::MAX) || named.ino()!=actual["inode"].as_u64().unwrap_or(u64::MAX) {return Err(refusal("sealed_snapshot_identity_changed"));}
    sync_directory(&directory)?;
    let absolute=fs::canonicalize(&read_path).map_err(io_error)?;
    let mut uri=String::from("file:");
    for byte in absolute.as_os_str().as_encoded_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte,b'/'|b'_'|b'-'|b'.') {uri.push(*byte as char);} else {uri.push_str(&format!("%{byte:02X}"));}
    }
    uri.push_str("?mode=ro&immutable=1");
    let db=Connection::open_with_flags(uri,OpenFlags::SQLITE_OPEN_READ_ONLY|OpenFlags::SQLITE_OPEN_URI|OpenFlags::SQLITE_OPEN_NOFOLLOW).map_err(sqlite_error)?;
    db.execute_batch("BEGIN;").map_err(sqlite_error)?;
    let current=fingerprint(&mut read_regular(&read_path)?)?;
    if current!=identity {return Err(refusal("private_reader_identity_changed"));}
    Ok(SealedReader{db,identity,read_path})
}

#[cfg(test)]
mod reader_tests {
    use super::*;
    use crate::store::import::fixture;

    #[test]
    fn sealed_sidecars_and_post_verify_replacement_refuse() {
        let root=std::env::temp_dir().join(format!("podmesh-sealed-reader-{}-{}",std::process::id(),crate::now()));
        let manifest=fixture::create(&root).unwrap();let caps=fixture::development_caps().unwrap();
        let captured=capture(&root.join("source.sqlite"),&manifest,&root.join("snapshot"),&caps).unwrap();
        for suffix in ["-wal","-shm","-journal"] {
            let companion=sidecar(&captured.path,suffix);create_file(&companion).unwrap();
            assert_eq!(open_sealed(&captured.path,&captured.seal).err().unwrap().message,"sealed_snapshot_sidecar_present");
            fs::remove_file(companion).unwrap();
        }
        let error=open_sealed_checked(&captured.path,&captured.seal,||{
            fs::rename(&captured.path,captured.path.with_extension("old")).unwrap();
            let mut replacement=create_file(&captured.path).unwrap();replacement.write_all(b"not-the-snapshot").unwrap();
        }).err().unwrap();
        assert_eq!(error.message,"sealed_snapshot_identity_changed");
        fs::remove_dir_all(root).unwrap();
    }
}
