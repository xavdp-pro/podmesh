//! Explicit new synthetic fixture generation, never an operational source or existing-file overwrite.
use super::{caps::Caps, refusal, snapshot, source};
use crate::store::{migrations, DurableStore, Result, SqliteStore, Value};
use rusqlite::Connection;
use serde_json::{json, Value as Json};
use std::{fs, path::Path};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

pub fn development_caps() -> Result<Caps> {
    Caps::parse(&serde_json::from_str(include_str!("../../../fixtures/C/copy-v1/resource-caps.json")).map_err(|_| refusal("invalid_resource_caps"))?)
}

pub fn create(directory: &Path) -> Result<Json> {
    fs::DirBuilder::new().mode(0o700).create(directory).map_err(|_| refusal("fixture_directory_must_be_new"))?;
    fs::set_permissions(directory,fs::Permissions::from_mode(0o700)).map_err(|_| refusal("fixture_permissions_failed"))?;
    let directory=fs::canonicalize(directory).map_err(|_| refusal("fixture_path_failed"))?;
    let path=directory.join("source.sqlite");
    snapshot::create_file(&path)?;
    let lock=directory.join("source.lock"); snapshot::create_file(&lock)?;
    let conn=Connection::open(&path).map_err(|_| refusal("fixture_sqlite_open_failed"))?;
    let mut store=SqliteStore::adopt(conn);
    migrations::apply_set(&mut store,migrations::NODE,&migrations::NODE_MIGRATIONS[..source::SOURCE_VERSION])?;
    store.execute("UPDATE store_schema SET applied_at=1 WHERE name='node'",&[])?;
    let shape=source::shape(store.connection())?;
    for (table_index,(name,definition)) in shape["tables"].as_object().ok_or_else(|| refusal("fixture_shape_failed"))?.iter().enumerate() {
        if name=="store_schema" {continue;}
        let definitions=definition["columns"].as_array().ok_or_else(|| refusal("fixture_shape_failed"))?;
        let columns:Vec<String>=definitions.iter().map(|d|d[1].as_str().unwrap().to_string()).collect();
        let template=source::Table{name:name.clone(),columns,rows:Vec::new(),hash:String::new()};
        let pk_count=definitions.iter().filter(|d|d[5].as_i64().unwrap_or(0)>0).count();
        for row in 0..3 {
            let values:Vec<Value>=definitions.iter().enumerate().map(|(i,d)|{
                let pk=d[5].as_i64().unwrap_or(0)>0;
                if d[3]==0 && !pk && row==0 {return Value::Null;}
                match d[2].as_str().unwrap() {
                    "INTEGER" if pk && pk_count==1=>Value::Integer([-10,0,10][row]),
                    "INTEGER"=>Value::Integer([i64::MIN,0,i64::MAX][row]),
                    "TEXT"=>Value::Text(format!("fx-{table_index}-{row}-{i}-é中🙂e\u{301};'")),
                    _=>unreachable!("pinned node source only TEXT/INTEGER"),
                }
            }).collect();
            store.execute(&template.insert()?,&values)?;
        }
    }
    for key in ["CaseProbe","caseprobe","cafe","café","é","e\u{301}","trail","trail "] {
        store.execute("INSERT INTO metadata(`key`,value) VALUES(?,?)",&[Value::Text(key.into()),Value::Text(format!("synthetic-F6-{key}"))])?;
    }
    for key in ["a".repeat(192),"🙂".repeat(192)] {
        store.execute("INSERT INTO metadata(`key`,value) VALUES(?,?)",&[Value::Text(key),Value::Text("synthetic-F6-width".into())])?;
    }
    for (key,value) in [("machine_id","synthetic-machine-no-adoption"),("host_uuid","synthetic-host-no-adoption"),("signed_policy_bytes"," {\"authority_key\":\"synthetic-public-only\", \"epoch\":1}\n")] {
        store.execute("INSERT INTO metadata(`key`,value) VALUES(?,?)",&[key.into(),value.into()])?;
    }
    for (id,intent) in [(11,"x".repeat(65)),(12," {\"action\":\"synthetic-replay\", \"ip\":\"192.0.2.1\", \"port\":8443, \"carrier\":\"synthetic-carrier\"} ".to_owned())] {
        store.execute("INSERT INTO network_effects(id,kind,`key`,owner,intent,state,operation_id,changed_at,observed) VALUES(?,?,?,?,?,?,?,?,?)",&[
            Value::Integer(id),"synthetic".into(),format!("synthetic-effect-{id}").into(),"synthetic-owner".into(),intent.into(),"synthetic-only".into(),format!("synthetic-operation-{id}").into(),1.into(),Value::Null,
        ])?;
    }
    let caps=development_caps()?;
    let size=fs::metadata(&path).map_err(|_| refusal("fixture_metadata_failed"))?.len();
    let plan=source::inspect(store.connection(),&caps,size)?;
    let tables:Vec<Json>=plan.tables.iter().map(source::Table::manifest).collect();
    drop(store);
    let manifest=json!({
        "generator":"podmesh-c02-synthetic/1","synthetic_only":true,"customer_data":false,"real_keys_or_credentials":false,
        "all_owners_and_maintenance_quiesced":true,"source_path":path,"source_lock":lock,"source_version":10,
        "tables":tables,"total_rows":plan.rows,"portable_columns":324,"source_shape_sha256":plan.shape_hash,
        "bundle":snapshot::bundle_manifest(&path,&caps)?,
        "scope":"Storage-only inert synthetic rows, not valid operational authority, keys, publisher effects or host adoption.",
        "F6_cases":["case","accent","NFC/NFD","trailing-space","ASCII192","Unicode192","intent65","complete-serialized-intent"],
        "kinds":"Node INTEGER/TEXT/NULL only; generic BLOB/REAL probes remain separate."
    });
    snapshot::private_json(&directory.join("source-manifest.json"),&manifest)?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all38_generator_snapshot_and_source_preservation_are_real() {
        let root=std::env::temp_dir().join(format!("podmesh-c02-offline-{}-{}",std::process::id(),crate::now()));
        let manifest=create(&root).unwrap();
        assert_eq!(manifest["tables"].as_array().unwrap().len(),38);
        assert!(manifest["tables"].as_array().unwrap().iter().all(|t|t["count"].as_u64().unwrap()>0));
        let caps=development_caps().unwrap(); let source_path=root.join("source.sqlite");
        let before=snapshot::bundle_manifest(&source_path,&caps).unwrap();
        let snapshot=snapshot::capture(&source_path,&manifest,&root.join("private-snapshot"),&caps).unwrap();
        assert!(snapshot::capture(&source_path,&manifest,&root.join("private-snapshot"),&caps).is_err());
        assert_eq!(snapshot::bundle_manifest(&source_path,&caps).unwrap(),before);
        let db=snapshot::open_sealed(&snapshot.path,&snapshot.seal).unwrap();
        let plan=source::inspect(&db,&caps,snapshot.seal["snapshot_bytes"].as_u64().unwrap()).unwrap();
        assert_eq!(plan.tables.iter().map(source::Table::manifest).collect::<Vec<_>>(),manifest["tables"].as_array().unwrap().clone());
        assert_eq!(plan.rows,129);
        drop(db);
        // Fixture-local test cleanup only; campaign evidence is persisted by the future harness.
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn committed_wal_is_included_and_uncommitted_spill_is_excluded() {
        let root=std::env::temp_dir().join(format!("podmesh-c02-wal-{}-{}",std::process::id(),crate::now()));
        let mut manifest=create(&root).unwrap();
        let source_path=root.join("source.sqlite");
        // This is the synthetic fixture owner, explicitly paused until capture finishes.
        let owner=Connection::open(&source_path).unwrap();
        owner.execute_batch("PRAGMA journal_mode=WAL; PRAGMA cache_size=1;").unwrap();
        owner.execute("INSERT INTO metadata VALUES('committed-wal','synthetic')",[]).unwrap();
        owner.execute_batch("BEGIN IMMEDIATE;").unwrap();
        owner.execute("INSERT INTO observations(id,observed_at,operation,result) VALUES(90,1,'uncommitted',?)",["dummy".repeat(30000)]).unwrap();
        let caps=development_caps().unwrap();
        manifest["bundle"]=snapshot::bundle_manifest(&source_path,&caps).unwrap();
        assert!(!manifest["bundle"]["wal"].is_null());
        let snapshot=snapshot::capture(&source_path,&manifest,&root.join("private-snapshot"),&caps).unwrap();
        assert_eq!(snapshot::bundle_manifest(&source_path,&caps).unwrap(),manifest["bundle"]);
        let db=snapshot::open_sealed(&snapshot.path,&snapshot.seal).unwrap();
        assert_eq!(db.query_row("SELECT count(*) FROM metadata WHERE key='committed-wal'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
        assert_eq!(db.query_row("SELECT count(*) FROM observations WHERE id=90",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        assert_eq!(source::inspect(&db,&caps,snapshot.seal["snapshot_bytes"].as_u64().unwrap()).unwrap().rows,130);
        drop(db); owner.execute_batch("ROLLBACK").unwrap();drop(owner);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn drift_journal_symlink_and_caps_fail_before_any_clone_sqlite_open() {
        let root=std::env::temp_dir().join(format!("podmesh-c02-negative-{}-{}",std::process::id(),crate::now()));
        let manifest=create(&root).unwrap();let path=root.join("source.sqlite");let caps=development_caps().unwrap();
        let target=root.join("never-created");
        let mut tiny=caps.clone();tiny.bundle=1;
        assert_eq!(snapshot::capture(&path,&manifest,&target,&tiny).err().unwrap().message,"source_limit_exceeded");
        assert!(!target.exists());
        std::os::unix::fs::symlink(&path,root.join("alias.sqlite")).unwrap();
        let mut aliased=manifest.clone();aliased["source_path"]=json!(root.join("alias.sqlite"));
        assert!(snapshot::capture(&root.join("alias.sqlite"),&aliased,&target,&caps).is_err());
        assert!(!target.exists());
        snapshot::create_file(&root.join("source.sqlite-journal")).unwrap();
        assert_eq!(snapshot::capture(&path,&manifest,&target,&caps).err().unwrap().message,"rollback_journal_source_refused");
        assert!(!target.exists());
        fs::remove_file(root.join("source.sqlite-journal")).unwrap();
        let mut changed=manifest.clone();changed["bundle"]["main"]["sha256"]=json!("bad");
        assert_eq!(snapshot::capture(&path,&changed,&target,&caps).err().unwrap().message,"source_bundle_manifest_mismatch");
        assert!(!target.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resource_caps_and_source_classes_refuse_before_target_work() {
        let root=std::env::temp_dir().join(format!("podmesh-c02-input-limits-{}-{}",std::process::id(),crate::now()));
        create(&root).unwrap();let db=Connection::open(root.join("source.sqlite")).unwrap();
        let size=fs::metadata(root.join("source.sqlite")).unwrap().len();let caps=development_caps().unwrap();
        for kind in ["snapshot","totalbytes","rows","tablebytes","tablerows","valuebytes","memory"] {
            let mut limited=caps.clone();
            match kind {"snapshot"=>limited.snapshot=1,"totalbytes"=>limited.scalar_bytes=1,"rows"=>limited.rows=1,"tablebytes"=>limited.table_bytes=1,"tablerows"=>limited.table_rows=1,"valuebytes"=>limited.value_bytes=1,"memory"=>limited.peak=1,_=>unreachable!()}
            assert_eq!(source::inspect(&db,&limited,size).err().unwrap().message,"source_limit_exceeded", "{kind}");
        }
        for (value,code) in [("CAST(x'80' AS TEXT)","invalid_utf8_text"),("x'80'","unsupported_storage_class_blob")]{
            db.execute_batch(&format!("UPDATE metadata SET value={value} WHERE key='CaseProbe';")).unwrap();
            assert_eq!(source::inspect(&db,&caps,size).err().unwrap().message,code);
        }
        db.execute("UPDATE metadata SET value='synthetic' WHERE key='CaseProbe'",[]).unwrap();
        db.execute("INSERT INTO metadata VALUES(NULL,'synthetic')",[]).unwrap();
        assert_eq!(source::inspect(&db,&caps,size).err().unwrap().message,"null_in_nonnullable_column");
        drop(db);fs::remove_dir_all(root).unwrap();
    }
}
