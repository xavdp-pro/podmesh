use std::{
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::PathBuf,
    time::Duration,
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // `podmeshd control-relay <socket>`: one request from stdin to a Unix socket, its reply to
    // stdout. The daemon runs this copy of itself inside a manager universe's PID namespace
    // (`nsenter -p`), because the resident checks the connecting peer's credentials and a peer
    // whose PID is not visible from the universe is refused. No state, no journal, no argument
    // but the path: the relay carries bytes and decides nothing.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("control-relay") {
        return podmesh::control_relay(args.get(2).ok_or("control-relay needs the socket path")?);
    }
    let dir = PathBuf::from(std::env::var("PODMESH_STATE_DIR").unwrap_or("/var/lib/podmesh".into()));
    let socket = PathBuf::from(std::env::var("PODMESH_SOCKET").unwrap_or("/run/podmesh/api.sock".into()));
    let db = podmesh::open_state(&dir)?;
    // A connector whose lease lapsed while this daemon was down is still publishing: its unit is
    // systemd's. It is withdrawn first, connector and mark, in one journaled operation, before the
    // network reconciliation and before anything is served.
    match podmesh::withdraw_unentitled_publishers_at_startup(&db) {
        Ok(report) => eprintln!("PodMesh publisher withdrawal at startup: {report}"),
        Err(e) => eprintln!("PodMesh publisher withdrawal at startup FAILED: {e}; the reconciliation and the fence will retry it"),
    }
    // Whatever a crash left half-made on the network is undone before anything is served: an
    // effect that never became effective is never assumed. The report goes to the journal.
    match podmesh::reconcile_network(&db) {
        Ok(report) => eprintln!("PodMesh network reconciliation at startup: {report}"),
        Err(e) => eprintln!("PodMesh network reconciliation at startup FAILED: {e}; network mutations will refuse until it succeeds"),
    }
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    std::fs::create_dir_all(socket.parent().ok_or("Invalid socket path")?)?;
    // Refuse an existing endpoint rather than unlink another service's socket.
    let listener = UnixListener::bind(&socket)?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    eprintln!("PodMesh local API ready: {}", socket.display());
    for stream in listener.incoming() {
        let mut stream = stream?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        let mut bytes = Vec::new();
        let mut byte = [0];
        let response = loop {
            match stream.read(&mut byte) {
                Ok(0) => break serde_json::json!({"ok":false,"error":"Incomplete request"}),
                Ok(_) => {
                    if byte[0] == b'\n' {
                        break match serde_json::from_slice(&bytes) {
                            Ok(v) => podmesh::handle(&db, &v),
                            Err(_) => serde_json::json!({"ok":false,"error":"Invalid JSON"}),
                        };
                    }
                    bytes.push(byte[0]);
                    if bytes.len() > 4096 {
                        break serde_json::json!({"ok":false,"error":"Request too large"});
                    }
                }
                Err(_) => break serde_json::json!({"ok":false,"error":"Request timeout"}),
            }
        };
        let _ = writeln!(stream, "{response}");
    }
    Ok(())
}
