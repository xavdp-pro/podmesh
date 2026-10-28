//! MeshFS — Pure Rust Sovereign Distributed Storage Engine and CLI
//! With high-speed Unix Domain Socket control, deep integrity inspection,
//! and 100% Proxmox VE Plugin compatibility.

use clap::Parser;
use meshfs::cli::{Cli, Commands, VolumeSubcommand};
use meshfs::control::{query_control_socket, start_control_socket, ControlRequest};
use meshfs::engine::MeshFSEngine;
use meshfs::server::create_router;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    let args = Cli::parse();

    // Configure logging filter based on --debug flag or RUST_LOG environment
    let default_level = if args.debug { "debug" } else { "info" };
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("meshfs={},tower_http=info", default_level)));

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .with_thread_ids(false)
        .init();

    match args.command {
        Commands::Mount(m) => {
            return match std::thread::spawn(move || meshfs::fuse::mount(m.peer, m.volume, &m.mountpoint))
                .join()
            {
                Ok(Ok(())) => ExitCode::SUCCESS,
                other => {
                    error!("mount failed: {:?}", other);
                    ExitCode::FAILURE
                }
            }
        }
        Commands::Daemon(d) => {
            info!(
                cluster = %d.cluster,
                data_dir = %d.data_dir.display(),
                socket = %d.socket.display(),
                port = d.port,
                "Démarrage du démon MeshFS Pure Rust v2.3.0"
            );

            let engine = match MeshFSEngine::new(&d.data_dir, &d.cluster, d.port).await {
                Ok(eng) => eng,
                Err(e) => {
                    error!("Erreur initialisation moteur: {}", e);
                    return ExitCode::FAILURE;
                }
            };

            if let Some(path) = &d.cluster_config {
                let config = match meshfs::cluster::ClusterConfig::load(path) {
                    Ok(c) => c,
                    Err(e) => {
                        error!("Cluster configuration: {}", e);
                        return ExitCode::FAILURE;
                    }
                };
                match meshfs::cluster::Cluster::start(engine.clone(), config, d.bootstrap).await {
                    Ok(c) => {
                        let _ = engine.cluster.set(c);
                    }
                    Err(e) => {
                        error!("Cluster startup: {}", e);
                        return ExitCode::FAILURE;
                    }
                }
            } else if d.data_dir.join("raft").exists() {
                error!("Existing clustered data requires --cluster-config; refusing standalone startup");
                return ExitCode::FAILURE;
            }

            // Start local Unix Domain Socket listener for zero-latency control
            if let Err(e) = start_control_socket(engine.clone(), d.socket.clone()).await {
                error!(path = %d.socket.display(), "Impossible d'initialiser le socket Unix de contrôle: {}", e);
            } else {
                info!(socket = %d.socket.display(), "Socket de contrôle local IPC actif");
            }

            let app = create_router(engine);
            let addr = format!("0.0.0.0:{}", d.port);
            let listener = match tokio::net::TcpListener::bind(&addr).await {
                Ok(l) => l,
                Err(e) => {
                    error!("Impossible d'écouter sur {}: {}", addr, e);
                    return ExitCode::FAILURE;
                }
            };

            info!(address = %addr, "Écoute active sur HTTP (Compatible Proxmox VE Plugin sur /api/v1/meshfs/*)");

            if let Err(e) = axum::serve(listener, app).await {
                error!("Erreur serveur HTTP: {}", e);
                return ExitCode::FAILURE;
            }
        }

        Commands::Ping(p) => {
            if let Some(sock) = resolve_socket(p.socket.as_deref()) {
                match query_control_socket(&sock, &ControlRequest::Ping).await {
                    Ok(resp) => {
                        println!("{}", resp);
                        return ExitCode::SUCCESS;
                    }
                    Err(e) => {
                        error!("Erreur socket Unix ({}): {}", sock.display(), e);
                    }
                }
            }

            // Fallback to HTTP
            let client = reqwest::Client::new();
            let url = format!("{}/api/v1/meshfs/status", p.remote.trim_end_matches('/'));
            match client.get(&url).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        println!("{{\"status\":\"pong\",\"remote\":\"{}\"}}", url);
                        return ExitCode::SUCCESS;
                    }
                    eprintln!("Erreur statut distant: {}", resp.status());
                    return ExitCode::FAILURE;
                }
                Err(e) => {
                    eprintln!("Erreur connexion HTTP: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }

        Commands::Inspect(ins) => {
            if let Some(sock) = resolve_socket(ins.socket.as_deref()) {
                let req = ControlRequest::Inspect {
                    volume: ins.volume.clone(),
                    deep_sha_verify: ins.deep,
                };
                match query_control_socket(&sock, &req).await {
                    Ok(resp) => {
                        println!("{}", resp);
                        return ExitCode::SUCCESS;
                    }
                    Err(e) => {
                        error!("Erreur socket Unix ({}): {}", sock.display(), e);
                    }
                }
            }

            // Fallback to HTTP
            let client = reqwest::Client::new();
            let mut url = format!(
                "{}/api/v1/meshfs/diagnostics?deep={}",
                ins.remote.trim_end_matches('/'),
                ins.deep
            );
            if let Some(ref v) = ins.volume {
                url.push_str(&format!("&volume={}", urlencoding(v)));
            }

            match client.get(&url).send().await {
                Ok(resp) => match resp.text().await {
                    Ok(body) => println!("{}", body),
                    Err(e) => {
                        eprintln!("Erreur lecture réponse: {}", e);
                        return ExitCode::FAILURE;
                    }
                },
                Err(e) => {
                    eprintln!("Erreur inspection HTTP: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }

        Commands::Status(s) => {
            if let Some(sock) = resolve_socket(s.socket.as_deref()) {
                match query_control_socket(&sock, &ControlRequest::Status).await {
                    Ok(resp) => {
                        println!("{}", resp);
                        return ExitCode::SUCCESS;
                    }
                    Err(e) => {
                        error!("Erreur socket Unix ({}): {}", sock.display(), e);
                    }
                }
            }

            // Fallback to HTTP
            let client = reqwest::Client::new();
            let url = format!("{}/api/v1/meshfs/status", s.remote.trim_end_matches('/'));
            match client.get(&url).send().await {
                Ok(resp) => match resp.text().await {
                    Ok(body) => println!("{}", body),
                    Err(e) => {
                        eprintln!("Erreur lecture réponse: {}", e);
                        return ExitCode::FAILURE;
                    }
                },
                Err(e) => {
                    eprintln!("Erreur connexion MeshFS ({}): {}", url, e);
                    return ExitCode::FAILURE;
                }
            }
        }

        Commands::Join(j) => {
            let client = reqwest::Client::new();
            let url = format!(
                "{}/api/v1/meshfs/nodes/join",
                j.remote.trim_end_matches('/')
            );
            let payload = serde_json::json!({
                "targetAddress": j.target,
                "token": j.token
            });

            match client.post(&url).json(&payload).send().await {
                Ok(resp) => match resp.text().await {
                    Ok(body) => println!("{}", body),
                    Err(e) => {
                        eprintln!("Erreur lecture réponse: {}", e);
                        return ExitCode::FAILURE;
                    }
                },
                Err(e) => {
                    eprintln!("Erreur communication join ({}): {}", url, e);
                    return ExitCode::FAILURE;
                }
            }
        }

        Commands::Snapshot(sn) => {
            let client = reqwest::Client::new();
            let url = format!(
                "{}/api/v1/meshfs/snapshots",
                sn.remote.trim_end_matches('/')
            );
            let payload = serde_json::json!({
                "volume": sn.volume,
                "name": sn.name
            });

            match client.post(&url).json(&payload).send().await {
                Ok(resp) => match resp.text().await {
                    Ok(body) => println!("{}", body),
                    Err(e) => {
                        eprintln!("Erreur lecture réponse: {}", e);
                        return ExitCode::FAILURE;
                    }
                },
                Err(e) => {
                    eprintln!("Erreur création snapshot ({}): {}", url, e);
                    return ExitCode::FAILURE;
                }
            }
        }

        Commands::Volume(v) => match v.action {
            VolumeSubcommand::List { remote } => {
                let client = reqwest::Client::new();
                let url = format!("{}/api/v1/meshfs/volumes", remote.trim_end_matches('/'));
                match client.get(&url).send().await {
                    Ok(resp) => match resp.text().await {
                        Ok(body) => println!("{}", body),
                        Err(e) => {
                            eprintln!("Erreur lecture: {}", e);
                            return ExitCode::FAILURE;
                        }
                    },
                    Err(e) => {
                        eprintln!("Erreur volumes ({}): {}", url, e);
                        return ExitCode::FAILURE;
                    }
                }
            }
            VolumeSubcommand::Create { name, remote } => {
                let client = reqwest::Client::new();
                let url = format!("{}/api/v1/meshfs/volumes", remote.trim_end_matches('/'));
                let payload = serde_json::json!({ "name": name });
                match client.post(&url).json(&payload).send().await {
                    Ok(resp) => match resp.text().await {
                        Ok(body) => println!("{}", body),
                        Err(e) => {
                            eprintln!("Erreur: {}", e);
                            return ExitCode::FAILURE;
                        }
                    },
                    Err(e) => {
                        eprintln!("Erreur création volume: {}", e);
                        return ExitCode::FAILURE;
                    }
                }
            }
        },

        Commands::Put(p) => {
            let file = match tokio::fs::File::open(&p.file).await {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("Cannot read source: {}", e);
                    return ExitCode::FAILURE;
                }
            };
            let client = reqwest::Client::new();
            let response = client
                .post(format!(
                    "{}/api/v1/meshfs/files/stream",
                    p.remote.trim_end_matches('/')
                ))
                .query(&[("volume", &p.volume), ("path", &p.path)])
                .body(reqwest::Body::wrap_stream(
                    tokio_util::io::ReaderStream::new(file),
                ))
                .send()
                .await;
            match response {
                Ok(r) => {
                    let status = r.status();
                    println!("{}", r.text().await.unwrap_or_default());
                    if !status.is_success() {
                        return ExitCode::FAILURE;
                    }
                }
                Err(e) => {
                    eprintln!("Upload failed: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }

        Commands::Get(g) => {
            let client = reqwest::Client::new();
            let url = format!(
                "{}/api/v1/meshfs/files/download?volume={}&path={}",
                g.remote.trim_end_matches('/'),
                urlencoding(&g.volume),
                urlencoding(&g.path)
            );

            let mut response = match client.get(&url).send().await {
                Ok(r) if r.status().is_success() => r,
                Ok(r) => {
                    eprintln!("Download failed: {}", r.status());
                    return ExitCode::FAILURE;
                }
                Err(e) => {
                    eprintln!("Download failed: {}", e);
                    return ExitCode::FAILURE;
                }
            };
            let expected = response
                .headers()
                .get(reqwest::header::ETAG)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.trim_matches('"').to_string());
            let temporary = g
                .output
                .as_ref()
                .map(|p| p.with_extension(format!("meshfs-download-{}", std::process::id())));
            let mut output: Box<dyn Write> = if let Some(path) = &temporary {
                match fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
                {
                    Ok(f) => Box::new(f),
                    Err(e) => {
                        eprintln!("Output: {}", e);
                        return ExitCode::FAILURE;
                    }
                }
            } else {
                Box::new(io::stdout())
            };
            use sha2::{Digest, Sha256};
            let mut hash = Sha256::new();
            loop {
                match response.chunk().await {
                    Ok(Some(bytes)) => {
                        hash.update(&bytes);
                        if let Err(e) = output.write_all(&bytes) {
                            eprintln!("Output: {}", e);
                            return ExitCode::FAILURE;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        eprintln!("Incomplete download: {}", e);
                        return ExitCode::FAILURE;
                    }
                }
            }
            if expected
                .as_ref()
                .is_some_and(|e| e != &format!("sha256:{}", hex::encode(hash.finalize())))
            {
                eprintln!("Download digest mismatch");
                return ExitCode::FAILURE;
            }
            if let Err(e) = output.flush() {
                eprintln!("Output: {}", e);
                return ExitCode::FAILURE;
            }
            drop(output);
            if let (Some(tmp), Some(path)) = (temporary, g.output) {
                if let Err(e) = fs::File::open(&tmp)
                    .and_then(|f| f.sync_all())
                    .and_then(|_| fs::rename(&tmp, &path))
                {
                    eprintln!("Output commit: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
    }

    ExitCode::SUCCESS
}

/// Automatically detects existing Unix control socket if not explicitly specified.
fn resolve_socket(custom: Option<&Path>) -> Option<std::path::PathBuf> {
    if let Some(p) = custom {
        return Some(p.to_path_buf());
    }
    let default_path = Path::new("/var/lib/meshfs/meshfs.sock");
    if default_path.exists() {
        return Some(default_path.to_path_buf());
    }
    None
}

fn urlencoding(s: &str) -> String {
    let mut encoded = String::new();
    for byte in s.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    encoded
}
