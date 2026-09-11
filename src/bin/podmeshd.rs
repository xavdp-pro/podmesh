use std::{io::{Read,Write},os::unix::{net::UnixListener,fs::PermissionsExt},path::PathBuf,time::Duration};
fn main()->Result<(),Box<dyn std::error::Error>>{
 let dir=PathBuf::from(std::env::var("PODMESH_STATE_DIR").unwrap_or("/var/lib/podmesh".into()));
 let socket=PathBuf::from(std::env::var("PODMESH_SOCKET").unwrap_or("/run/podmesh/api.sock".into()));
 let db=podmesh::open_state(&dir)?;
 std::fs::set_permissions(&dir,std::fs::Permissions::from_mode(0o700))?;
 std::fs::create_dir_all(socket.parent().ok_or("Invalid socket path")?)?;
 // Refuse an existing endpoint rather than unlink another service's socket.
 let listener=UnixListener::bind(&socket)?;
 std::fs::set_permissions(&socket,std::fs::Permissions::from_mode(0o600))?;
 eprintln!("PodMesh local API ready: {}",socket.display());
 for stream in listener.incoming(){let mut stream=stream?;stream.set_read_timeout(Some(Duration::from_secs(2)))?;stream.set_write_timeout(Some(Duration::from_secs(2)))?;
 let mut bytes=Vec::new();let mut byte=[0];let response=loop{
 match stream.read(&mut byte){Ok(0)=>break serde_json::json!({"ok":false,"error":"Incomplete request"}),Ok(_)=>{if byte[0]==b'\n'{break match serde_json::from_slice(&bytes){Ok(v)=>podmesh::handle(&db,&v),Err(_)=>serde_json::json!({"ok":false,"error":"Invalid JSON"})};}bytes.push(byte[0]);if bytes.len()>4096{break serde_json::json!({"ok":false,"error":"Request too large"});}},Err(_)=>break serde_json::json!({"ok":false,"error":"Request timeout"})}};
 let _=writeln!(stream,"{response}");
 }Ok(())
}
