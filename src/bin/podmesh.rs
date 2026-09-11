use std::{io::{Read,Write},os::unix::net::UnixStream,time::Duration};
fn main()->Result<(),Box<dyn std::error::Error>>{
 let operation=std::env::args().nth(1).unwrap_or("capabilities".into());
 let socket=std::env::var("PODMESH_SOCKET").unwrap_or("/run/podmesh/api.sock".into());
 let mut stream=UnixStream::connect(socket)?;stream.set_read_timeout(Some(Duration::from_secs(400)))?;// Clone commits may take up to 300 s.
 let mut request = if let Some(path)=std::env::args().nth(2) {serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(path)?)?}else{serde_json::json!({})};
 if !request.is_object(){return Err("Request must be a JSON object".into());}
 request["operation"]=serde_json::json!(operation);
 writeln!(stream,"{request}")?;
 let mut response=String::new();stream.read_to_string(&mut response)?;
 let value:serde_json::Value=serde_json::from_str(&response)?;
 println!("{}",serde_json::to_string_pretty(&value)?);
 if value["ok"]!=true{std::process::exit(1);}Ok(())
}
