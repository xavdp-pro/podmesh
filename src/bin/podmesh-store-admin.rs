//! Explicit private-root schema setup, never functional startup or data cutover.
use podmesh::store::{self, migrations, DurableStore, Engine, StoreConfig};
use serde_json::{json,Value};
use std::{fs,io::Read,path::{Path,PathBuf},os::unix::fs::{MetadataExt,PermissionsExt,OpenOptionsExt,FileTypeExt}};
type Error=Box<dyn std::error::Error>;
const USAGE:&str="Usage: podmesh-store-admin prepare --role node|manager --profile ROOT_PROFILE --admin-socket PRIVATE_UNIX_SOCKET --admin-password-file ROOT0600_FILE\nRequires real uid0 and euid0, an existing empty role database or an exact-current schema, and no other non-daemon server connections. No TCP/DSN, arbitrary SQL, grants, copy, drop, reset, force or old/partial-schema repair. Functional and private admin socket paths must identify the same local socket.";
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
enum Role {Node,Manager}
impl Role {
    fn parse(s:&str)->Result<Self,Error>{match s {"node"=>Ok(Self::Node),"manager"=>Ok(Self::Manager),_=>Err("role must be node or manager".into())}}
    fn slug(self)->&'static str{match self{Self::Node=>"podmesh-node",Self::Manager=>"podmesh-manager"}}
    fn verify(self,db:&mut dyn DurableStore)->store::Result<()>{match self{Self::Node=>migrations::verify_current_node(db),Self::Manager=>migrations::verify_current_manager(db)}}
    fn apply(self,db:&mut dyn DurableStore)->store::Result<migrations::Applied>{match self{Self::Node=>migrations::apply(db),Self::Manager=>migrations::apply_manager(db)}}
}
struct Args {role:Role,profile:PathBuf,socket:PathBuf,password:PathBuf}
fn parse(args:Vec<String>)->Result<Args,Error>{
    if args.first().map(String::as_str)!=Some("prepare") {return Err(USAGE.into());}
    let mut role=None;let mut profile=None;let mut socket=None;let mut password=None;
    let mut i=1;while i<args.len(){
        let value=args.get(i+1).filter(|v|!v.starts_with("--")).ok_or("missing schema-admin option value")?;
        match args[i].as_str(){
            "--role" if role.is_none()=>role=Some(Role::parse(value)?),
            "--profile" if profile.is_none()=>profile=Some(PathBuf::from(value)),
            "--admin-socket" if socket.is_none()=>socket=Some(PathBuf::from(value)),
            "--admin-password-file" if password.is_none()=>password=Some(PathBuf::from(value)),
            _=>return Err("unknown or duplicate schema-admin option".into()),
        } i+=2;
    }
    Ok(Args{role:role.ok_or("role is required")?,profile:profile.ok_or("profile is required")?,socket:socket.ok_or("admin socket is required")?,password:password.ok_or("separate private admin password file is required")?})
}
fn require_root(uid:u32)->Result<(),Error>{if uid!=0{Err("private schema administration requires real uid0 and euid0".into())}else{Ok(())}}
fn absolute(path:&Path)->Result<(),Error>{
    if !path.is_absolute() || path.components().any(|c|!matches!(c,std::path::Component::RootDir|std::path::Component::Normal(_))) {
        return Err("schema administration requires normalized absolute paths".into());
    }Ok(())
}
fn root_parents(path:&Path)->Result<(),Error>{
    absolute(path)?;let mut p=path.parent();
    while let Some(dir)=p {
        let m=fs::symlink_metadata(dir)?;
        if !m.is_dir()||m.uid()!=0||m.permissions().mode()&0o022!=0 {return Err("schema-admin path parents must be protected root-owned directories".into());}
        p=dir.parent();
    }Ok(())
}
fn file_identity(m:&fs::Metadata)->(u64,u64,i64,i64){(m.dev(),m.ino(),m.ctime(),m.ctime_nsec())}
fn root_file(path:&Path,private:bool,maximum:usize)->Result<Vec<u8>,Error>{
    root_parents(path)?;let before=fs::symlink_metadata(path)?;
    if !before.is_file()||before.uid()!=0||before.permissions().mode()&0o022!=0
        || (private&&before.permissions().mode()&0o777!=0o600){return Err("schema-admin file must be protected root-owned regular file; credential mode0600".into());}
    let file=fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC|libc::O_NONBLOCK).open(path)?;
    if file_identity(&before)!=file_identity(&file.metadata()?){return Err("schema-admin file changed during open".into());}
    let mut bytes=Vec::new();(&file).take(maximum as u64+1).read_to_end(&mut bytes)?;
    if bytes.len()>maximum||file_identity(&before)!=file_identity(&file.metadata()?)
        ||file_identity(&before)!=file_identity(&fs::symlink_metadata(path)?){return Err("schema-admin file oversized or replaced".into());}
    Ok(bytes)
}
fn socket_identity(path:&Path)->Result<(u64,u64),Error>{
    absolute(path)?;let m=fs::symlink_metadata(path)?;
    if !m.file_type().is_socket(){return Err("declared local endpoint is not a Unix socket".into());}Ok((m.dev(),m.ino()))
}
fn profile(document:&Value,role:Role)->Result<StoreConfig,Error>{
    if document.get("store").is_some() && document.as_object().map_or(true,|o|o.len()!=1) {
        return Err("wrapped profile must contain only store".into());
    }
    let d=document.get("store").unwrap_or(document);
    let object=d.as_object().ok_or("functional profile is not an object")?;
    if object.keys().any(|k|!matches!(k.as_str(),"engine"|"mariadb"))||d["engine"]!="mariadb" {return Err("explicit MariaDB profile without DSN/SQLite fallback is required".into());}
    let sql=d["mariadb"].as_object().ok_or("MariaDB fields missing")?;
    if sql.keys().any(|k|!matches!(k.as_str(),"user"|"database"|"socket"|"password_file"|"host"|"port"|"connect_timeout_ms"|"lock_wait_timeout_seconds")) {return Err("unknown MariaDB administrative profile field or DSN".into());}
    if d["mariadb"]["user"]!=role.slug()||d["mariadb"]["database"]!=role.slug() {
        return Err("functional user/database must be explicit and equal the role slug".into());
    }
    let config=StoreConfig::from_json(d,None).map_err(|_|"functional profile refused")?;
    if config.engine!=Engine::Mariadb||config.mariadb.dsn.is_some()||config.mariadb.user!=role.slug()||config.mariadb.database!=role.slug() {
        return Err("profile must preserve exact declared role slug/user/database".into());
    }
    if config.mariadb.socket.is_none()||config.mariadb.password_file.is_none(){return Err("functional Unix socket and credential file must be explicit".into());}
    absolute(config.mariadb.socket.as_deref().unwrap())?;absolute(config.mariadb.password_file.as_deref().unwrap())?;
    Ok(config)
}
fn administer(role:Role,db:&mut dyn DurableStore)->Result<&'static str,Error>{
    // Offline is a deployment precondition: refuse every other non-daemon connection,
    // including sessions that could use qualified names without selecting this DB.
    // Functional startup cannot become current until the final immutable migration
    // is recorded; this entry never repairs a nonempty old/interrupted journal.
    let peers=db.query_one("SELECT COUNT(*) FROM information_schema.PROCESSLIST WHERE `ID`<>CONNECTION_ID() AND `COMMAND`<>'Daemon'",&[])?
        .ok_or("offline connection inventory unreadable")?.integer(0)?;
    if peers!=0{return Err("database is not offline: other connections are present".into());}
    if db.tables()?.is_empty(){role.apply(db)?;role.verify(db)?;Ok("prepared_empty_schema")}
    else{role.verify(db)?;Ok("already_current_no_ddl")}
}
fn run()->Result<(),Error>{
    // No CLI flag, ambient DSN or caller assertion can supply root authority.
    require_root(unsafe{libc::getuid()})?;
    require_root(unsafe{libc::geteuid()})?;
    let args=parse(std::env::args().skip(1).collect())?;
    if !cfg!(feature="mariadb"){return Err("schema-admin requires a build with the mariadb feature".into());}
    let document:Value=serde_json::from_slice(&root_file(&args.profile,false,65536)?).map_err(|_|"root profile JSON is malformed")?;
    let mut config=profile(&document,args.role)?;
    root_parents(&args.socket)?;
    let parent=fs::symlink_metadata(args.socket.parent().ok_or("admin socket parent missing")?)?;
    if parent.permissions().mode()&0o777!=0o700 {return Err("admin socket must have a private root0700 parent".into());}
    let socket=socket_identity(&args.socket)?;
    let functional_socket=config.mariadb.socket.clone().unwrap();
    if socket_identity(&functional_socket)?!=socket{return Err("functional and private admin paths must identify the same local server socket".into());}
    let credential=root_file(&args.password,true,4096)?;
    if credential.is_empty()||credential.iter().all(u8::is_ascii_whitespace) {return Err("private administrator credential is empty".into());}
    let functional_password=config.mariadb.password_file.as_ref().unwrap();
    let functional=fs::symlink_metadata(functional_password)?;let admin=fs::symlink_metadata(&args.password)?;
    if !functional.is_file()||(functional.dev(),functional.ino())==(admin.dev(),admin.ino()) {
        return Err("administrator authentication must be separate from the functional credential".into());
    }
    config.mariadb.user="root".into();config.mariadb.password_file=Some(args.password);config.mariadb.socket=Some(args.socket.clone());
    config.mariadb.host="localhost".into();config.mariadb.dsn=None;
    config.validate().map_err(|_|"private root socket credentials refused")?;
    // mysql-28 explicit socket chooses connect_socket, not TCP fallback. This
    // existing backend still enforces isolation/timeouts/flush durability.
    let mut db=store::open(&config).map_err(|_|"private root Unix-socket connection failed")?;
    if socket_identity(&args.socket)?!=socket||socket_identity(&functional_socket)?!=socket{return Err("declared socket identity changed while connecting".into());}
    let target=db.query_one("SELECT DATABASE(), CURRENT_USER()",&[]).map_err(|_|"administrator identity query failed")?
        .ok_or("administrator identity unavailable")?;
    if target.text(0)?!=args.role.slug()||!target.text(1)?.starts_with("root@") {return Err("connected database/administrator identity differs from declared role".into());}
    let status=administer(args.role,db.as_mut())?;
    println!("{}",json!({"status":status,"role":match args.role{Role::Node=>"node",Role::Manager=>"manager"},"database":args.role.slug(),"transport":"explicit-local-unix-socket","data_copy":false}));
    Ok(())
}
fn main(){if let Err(error)=run(){eprintln!("schema-admin refused: {error}");std::process::exit(1);}}

#[cfg(test)]
mod tests {
    use super::*;
    // This fake refuses every mutation; these cases must never reach apply.
    struct Existing {peers:i64,version:i64}
    impl DurableStore for Existing {
        fn engine(&self)->Engine{Engine::Mariadb}
        fn execute(&mut self,_:&str,_:&[store::Value])->store::Result<u64>{panic!("admin mutated existing schema")}
        fn execute_batch(&mut self,_:&str)->store::Result<()>{panic!("admin ran DDL on existing schema")}
        fn transaction(&mut self)->store::Result<Box<dyn store::Transaction+'_>>{panic!("unexpected admin transaction")}
        fn query(&mut self,sql:&str,_:&[store::Value])->store::Result<Vec<store::Row>>{
            use std::sync::Arc;
            if sql=="SELECT COUNT(*) FROM information_schema.PROCESSLIST WHERE `ID`<>CONNECTION_ID() AND `COMMAND`<>'Daemon'" {
                return Ok(vec![store::Row::new(Arc::new(vec!["count".into()]),vec![store::Value::Integer(self.peers)])]);
            }
            assert_eq!(sql,"SELECT name, version FROM store_schema ORDER BY name");
            Ok(vec![store::Row::new(Arc::new(vec!["name".into(),"version".into()]),vec![store::Value::from("node"),store::Value::Integer(self.version)])])
        }
        fn integrity_check(&mut self)->store::Result<store::Integrity>{panic!("unexpected integrity check")}
        fn tables(&mut self)->store::Result<Vec<String>>{Ok(migrations::node_tables().into_iter().chain([store::SCHEMA_TABLE]).map(str::to_string).collect())}
    }
    #[test]
    fn private_schema_authority_admin_existing_schema_and_peers_never_reach_ddl(){
        assert!(administer(Role::Node,&mut Existing{peers:1,version:migrations::node_version()}).is_err());
        assert!(administer(Role::Node,&mut Existing{peers:0,version:0}).is_err());
        assert_eq!(administer(Role::Node,&mut Existing{peers:0,version:migrations::node_version()}).unwrap(),"already_current_no_ddl");
    }
    fn args()->Vec<String>{["prepare","--role","node","--profile","/root/profile.json","--admin-socket","/root/admin/db.sock","--admin-password-file","/root/admin/passwd"].into_iter().map(String::from).collect()}
    #[test]
    fn private_schema_authority_admin_parser_is_closed_and_root_guard_is_not_a_flag(){
        assert!(parse(args()).is_ok());assert!(require_root(1).is_err());assert!(require_root(0).is_ok());
        for flag in ["--force","--root","--sql","--to-dsn","--host"] {let mut a=args();a.push(flag.into());a.push("ignored".into());assert!(parse(a).is_err());}
        let mut duplicate=args();duplicate.extend(["--role".into(),"manager".into()]);assert!(parse(duplicate).is_err());
        let mut wrong=args();wrong[2]="other-role".into();assert!(parse(wrong).is_err());
        assert!(absolute(Path::new("relative.sock")).is_err());assert!(absolute(Path::new("/root/../socket")).is_err());
    }
    #[test]
    fn private_schema_authority_admin_profile_preserves_role_and_requires_unix_socket(){
        let valid=json!({"engine":"mariadb","mariadb":{"user":"podmesh-node","database":"podmesh-node","socket":"/run/node/db.sock","password_file":"/run/node/passwd"}});
        assert!(profile(&valid,Role::Node).is_ok());assert!(profile(&valid,Role::Manager).is_err());
        assert!(profile(&json!({"store":valid.clone()}),Role::Node).is_ok());
        assert!(profile(&json!({"store":valid.clone(),"extra":1}),Role::Node).is_err());
        for key in ["socket","password_file","user","database"] {let mut bad=valid.clone();bad["mariadb"].as_object_mut().unwrap().remove(key);assert!(profile(&bad,Role::Node).is_err());}
        let mut dsn=valid.clone();dsn["mariadb"]["dsn"]=json!("mysql://root:do-not-echo@host/database");assert!(profile(&dsn,Role::Node).is_err());
        let mut wrong=valid.clone();wrong["mariadb"]["database"]=json!("another-db");assert!(profile(&wrong,Role::Node).is_err());
        let mut wrong=valid.clone();wrong["mariadb"]["socket"]=json!("relative");assert!(profile(&wrong,Role::Node).is_err());
    }
}
