use std::{
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

extern "C" fn request_stop(_: libc::c_int) {
    // The signal handler must not touch the store, sockets, allocation or logging.
    STOP_REQUESTED.store(true, Ordering::Relaxed);
}

fn install_stop_handlers() -> std::io::Result<()> {
    for signal in [libc::SIGTERM, libc::SIGINT] {
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = request_stop as *const () as usize;
        // Let an admitted synchronous operation finish its real store/provider work.
        // Signals request termination; they do not manufacture a terminal result.
        action.sa_flags = libc::SA_RESTART;
        if unsafe { libc::sigemptyset(&mut action.sa_mask) } != 0
            || unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) } != 0
        {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

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
    // A container's PID 1 needs explicit handlers for ordinary termination.
    install_stop_handlers()?;
    let dir = PathBuf::from(std::env::var("PODMESH_STATE_DIR").unwrap_or("/var/lib/podmesh".into()));
    let socket = PathBuf::from(std::env::var("PODMESH_SOCKET").unwrap_or("/run/podmesh/api.sock".into()));
    // Which store carries this node's journal is a configuration, not a path (docs/STORE-CONFIGURATION.md).
    // A node with no profile keeps the journal it has, and one whose profile is incomplete refuses
    // to start rather than fall back to a file it was not configured to write.
    let profile = podmesh::store_profile(&dir)?;
    eprintln!("PodMesh store: {}", profile.described());
    if std::env::var_os(podmesh::host_adapter::SOCKET_ENV).is_some() {
        if profile.engine != podmesh::store::Engine::Mariadb || unsafe { libc::geteuid() } <= 1000 {
            return Err("private node application requires a non-system identity and explicit MariaDB profile".into());
        }
    }
    let mut store = podmesh::open_node_store(&dir, &profile)?;
    // Do not expose a lifecycle API over restored effects we cannot reconcile.
    store.validate_startup_scope()?;
    // A connector whose lease lapsed while this daemon was down is still publishing: its unit is
    // systemd's. It is withdrawn first, connector and mark, in one journaled operation, before the
    // network reconciliation and before anything is served.
    if let Some(db) = store.connection() {
        match podmesh::withdraw_unentitled_publishers_at_startup(db) {
            Ok(report) => eprintln!("PodMesh publisher withdrawal at startup: {report}"),
            Err(e) => eprintln!("PodMesh publisher withdrawal at startup FAILED: {e}; nothing retries it before the next start of this daemon but the fence, when something runs it (the reconciliation withdraws a recorded publisher, never an unrecorded connector)"),
        }
        // Whatever a crash left half-made on the network is undone before anything is served: an
        // effect that never became effective is never assumed. The report goes to the journal.
        match podmesh::reconcile_network(db) {
            Ok(report) => eprintln!("PodMesh network reconciliation at startup: {report}"),
            Err(e) => eprintln!("PodMesh network reconciliation at startup FAILED: {e}; network mutations will refuse until it succeeds"),
        }
    } else {
        eprintln!(
            "PodMesh durable lifecycle startup: publisher/network journal preconditions checked; \
             their reconciliation remains unavailable"
        );
    }
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    std::fs::create_dir_all(socket.parent().ok_or("Invalid socket path")?)?;
    // Refuse an existing endpoint rather than unlink another service's socket.
    if STOP_REQUESTED.load(Ordering::Relaxed) {
        return Ok(());
    }
    let listener = UnixListener::bind(&socket)?;
    listener.set_nonblocking(true)?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    eprintln!("PodMesh local API ready: {}", socket.display());
    while !STOP_REQUESTED.load(Ordering::Relaxed) {
        let mut stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        if STOP_REQUESTED.load(Ordering::Relaxed) {
            break;
        }
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        let mut bytes = Vec::new();
        let mut byte = [0];
        let response = loop {
            match stream.read(&mut byte) {
                Ok(0) => break serde_json::json!({"ok":false,"error":"Incomplete request"}),
                Ok(_) => {
                    if byte[0] == b'\n' {
                        if STOP_REQUESTED.load(Ordering::Relaxed) {
                            break serde_json::json!({"ok":false,"error":"Daemon stopping"});
                        }
                        break match serde_json::from_slice(&bytes) {
                            Ok(v) => store.handle(&v),
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
