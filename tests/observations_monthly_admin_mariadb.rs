//! NEW affected native case, authored only. Run explicitly on a separately leased CT.
//! Requires a fresh disposable canonical NODE MariaDB instance, never a production instance.
#![cfg(feature = "mariadb")]
use podmesh::store::{self, migrations, DurableStore, StoreConfig, Value as Stored};
use serde_json::{json,Value};
use std::{fs,path::PathBuf,process::Command};
type Error=Box<dyn std::error::Error>;
fn env(name:&str)->PathBuf{PathBuf::from(std::env::var(name).expect(name))}
fn snapshot(db:&mut dyn DurableStore)->Result<Vec<Vec<Stored>>,Error>{
    Ok(db.query("SELECT id, observed_at, operation, result FROM observations ORDER BY id",&[])?.into_iter().map(|r|r.into_values()).collect())
}
fn count(db:&mut dyn DurableStore,sql:&str)->Result<i64,Error>{Ok(db.query_one(sql,&[])?.ok_or("missing count")?.integer(0)?)}
#[test]
#[ignore = "explicit separately leased CT only; creates disposable NODE schema and DML; never run on P3"]
fn monthly_native_root_constructor_boundary_dml_rollover_and_global_id()->Result<(),Error>{
    assert_eq!(std::env::var("PODMESH_MONTHLY_TEST_SCOPE").as_deref(),Ok("EMPTY_DISPOSABLE_CANONICAL_NODE"));
    assert_eq!(unsafe{libc::getuid()},0);assert_eq!(unsafe{libc::geteuid()},0);
    let profile=env("PODMESH_MONTHLY_TEST_PROFILE");
    let socket=env("PODMESH_MONTHLY_TEST_ADMIN_SOCKET");
    let password=env("PODMESH_MONTHLY_TEST_ADMIN_PASSWORD_FILE");
    // Existing empty root0700 evidence directory, under protected root-owned parents.
    let record=env("PODMESH_MONTHLY_TEST_RECORD_DIR");
    assert!(record.is_absolute());assert!(fs::read_dir(&record)?.next().is_none());
    let first=record.join("initial-months.json");let next=record.join("advance-months.json");
    fs::write(&first,json!({"first_month":"2026-10","months":2}).to_string())?;
    fs::write(&next,json!({"first_month":"2026-12","months":1}).to_string())?;
    let invocation=std::cell::Cell::new(0_u32);
    let cli=|command:&str,plan:&PathBuf|->Result<bool,Error>{
        let index=invocation.get();invocation.set(index+1);
        let result=Command::new(env!("CARGO_BIN_EXE_podmesh-store-admin"))
            .args([command,"--role","node","--profile"]).arg(&profile)
            .arg("--admin-socket").arg(&socket).arg("--admin-password-file").arg(&password)
            .arg("--observations-months").arg(plan).output()?;
        // Logs are private qualification artifacts; never print credentials or DSNs.
        fs::write(record.join(format!("{index:02}-{command}-{}.stdout",plan.file_stem().unwrap().to_string_lossy())),&result.stdout)?;
        fs::write(record.join(format!("{index:02}-{command}-{}.stderr",plan.file_stem().unwrap().to_string_lossy())),&result.stderr)?;
        fs::write(record.join(format!("{index:02}-status.json")),json!({"command":command,"exit_code":result.status.code(),"success":result.status.success()}).to_string())?;
        Ok(result.status.success())
    };
    // Empty database is enforced by the actual root CLI before its first DDL.
    assert!(cli("prepare",&first)?);
    let document:Value=serde_json::from_slice(&fs::read(&profile)?)?;
    let config_doc=document.get("store").unwrap_or(&document);
    let app_config=StoreConfig::from_json(config_doc,None)?;
    assert_eq!(app_config.mariadb.database,"podmesh-node");
    assert_eq!(app_config.mariadb.user,"podmesh-node");
    let mut root_config=StoreConfig::from_json(config_doc,None)?;
    root_config.mariadb.user="root".into();root_config.mariadb.password_file=Some(password.clone());
    root_config.mariadb.socket=Some(socket.clone());root_config.mariadb.host="localhost".into();root_config.mariadb.dsn=None;
    let mut root=store::open(&root_config)?;
    migrations::verify_current_node(root.as_mut())?;
    let grants:Value=serde_json::from_str(include_str!("../packaging/podmesh-node/canonical-unit/grants.json"))?;
    // Fixture uses the exact current declared DML grants, no DDL privileges.
    for (table,rights) in grants["tables"].as_object().ok_or("grant manifest missing")? {
        assert!(table.chars().all(|c|c.is_ascii_alphanumeric()||c=='_'));
        let rights=rights.as_array().unwrap().iter().map(|v|v.as_str().unwrap()).collect::<Vec<_>>();
        assert!(rights.iter().all(|r|["SELECT","INSERT","UPDATE","DELETE"].contains(r)));
        root.execute_batch(&format!("GRANT {} ON `podmesh-node`.`{table}` TO 'podmesh-node'@'localhost';",rights.join(",")))?;
    }
    assert_eq!(count(root.as_mut(),"SELECT COUNT(*) FROM information_schema.PARTITIONS WHERE TABLE_SCHEMA=DATABASE() AND TABLE_NAME<>'observations' AND PARTITION_NAME IS NOT NULL")?,0);
    drop(root);
    let mut app=store::open(&app_config)?;
    let oct=1790812800_i64;let nov=1793491200_i64;let dec=1796083200_i64;let feb=1801440000_i64;
    for t in [oct-1,oct,nov-1,nov,dec,dec+1,feb]{
        app.execute("INSERT INTO observations(observed_at, operation, result) VALUES(?, ?, ?)",&[Stored::from(t),Stored::from("monthly-test"),Stored::from("retained")])?;
    }
    let before=snapshot(app.as_mut())?;assert_eq!(before.len(),7);
    assert!(before.windows(2).all(|w|w[0][0].integer().unwrap()<w[1][0].integer().unwrap()));
    // Functional account cannot perform even a harmless DDL change.
    let denied=app.execute_batch("ALTER TABLE observations COMMENT='forbidden-app-ddl';").unwrap_err();
    assert_eq!(denied.fault,store::Fault::Denied);
    app.execute("INSERT INTO operations(id,request,status,result) VALUES(?,?,'verified',?)",&[Stored::from("monthly-global-op"),Stored::from("original-request"),Stored::from("original-result")])?;
    // Another app session prevents maintenance before any partition change.
    assert!(!cli("advance-observations",&next)?);drop(app);
    assert!(cli("advance-observations",&next)?);
    // Constructor never converts a nonempty/current schema; old partitions remain intact.
    assert!(!cli("prepare",&first)?);
    let mut app=store::open(&app_config)?;
    assert_eq!(snapshot(app.as_mut())?,before);
    assert_eq!(count(app.as_mut(),"SELECT COUNT(*) FROM observations PARTITION (p_legacy)")?,1);
    assert_eq!(count(app.as_mut(),"SELECT COUNT(*) FROM observations PARTITION (p202610)")?,2);
    assert_eq!(count(app.as_mut(),"SELECT COUNT(*) FROM observations PARTITION (p202611)")?,1);
    assert_eq!(count(app.as_mut(),"SELECT COUNT(*) FROM observations PARTITION (p202612)")?,2);
    assert_eq!(count(app.as_mut(),"SELECT COUNT(*) FROM observations PARTITION (p_future)")?,1);
    // Late append and still-distant future append remain accepted after maintenance.
    app.execute("INSERT INTO observations(observed_at, operation, result) VALUES(?, ?, ?)",&[Stored::from(oct),Stored::from("late"),Stored::from("retained")])?;
    app.execute("INSERT INTO observations(observed_at, operation, result) VALUES(?, ?, ?)",&[Stored::from(feb),Stored::from("future"),Stored::from("retained")])?;
    assert_eq!(count(app.as_mut(),"SELECT COUNT(*) FROM observations")?,9);
    assert!(app.execute("INSERT INTO operations(id,request,status,result) VALUES(?,?,'verified',?)",&[Stored::from("monthly-global-op"),Stored::from("changed-request"),Stored::from("changed-result")]).is_err());
    let op=app.query_one("SELECT request,result FROM operations WHERE id=?",&[Stored::from("monthly-global-op")])?.unwrap();
    assert_eq!(op.text(0)?,"original-request");assert_eq!(op.text(1)?,"original-result");
    migrations::verify_current_node(app.as_mut())?;
    // Retain the owned database and evidence. No DROP, reset, cleanup deletion or restore replay.
    Ok(())
}
