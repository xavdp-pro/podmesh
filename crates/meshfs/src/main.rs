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
        Commands::Daemon(d) => {
            info!(
                cluster = %d.cluster,
                data_dir = %d.data_dir.display(),
                socket = %d.socket.display(),
                port = d.port,
                "Démarrage du démon MeshFS Pure Rust v2.1.0"
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
                    Ok(c) => c, Err(e) => { error!("Cluster configuration: {}", e); return ExitCode::FAILURE; }
                };
                match meshfs::cluster::Cluster::start(engine.clone(), config, d.bootstrap).await {
                    Ok(c) => { let _ = engine.cluster.set(c); },
                    Err(e) => { error!("Cluster startup: {}", e); return ExitCode::FAILURE; }
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
            let url = format!("{}/api/v1/meshfs/nodes/join", j.remote.trim_end_matches('/'));
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
            let url = format!("{}/api/v1/meshfs/snapshots", sn.remote.trim_end_matches('/'));
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
            let content = match fs::read(&p.file) {
                Ok(bytes) => bytes,
                Err(e) => {
                    eprintln!("Impossible de lire le fichier source '{}': {}", p.file.display(), e);
                    return ExitCode::FAILURE;
                }
            };

            // Encode content in base64
            const B64_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let mut b64 = String::with_capacity(content.len() * 4 / 3 + 4);
            let mut i = 0;
            while i < content.len() {
                let b0 = content[i] as usize;
                let b1 = if i + 1 < content.len() { content[i + 1] as usize } else { 0 };
                let b2 = if i + 2 < content.len() { content[i + 2] as usize } else { 0 };

                let trip = (b0 << 16) | (b1 << 8) | b2;

                b64.push(B64_CHARS[(trip >> 18) & 0x3F] as char);
                b64.push(B64_CHARS[(trip >> 12) & 0x3F] as char);
                if i + 1 < content.len() {
                    b64.push(B64_CHARS[(trip >> 6) & 0x3F] as char);
                } else {
                    b64.push('=');
                }
                if i + 2 < content.len() {
                    b64.push(B64_CHARS[trip & 0x3F] as char);
                } else {
                    b64.push('=');
                }
                i += 3;
            }

            let client = reqwest::Client::new();
            let url = format!("{}/api/v1/meshfs/files/upload", p.remote.trim_end_matches('/'));
            let payload = serde_json::json!({
                "volume": p.volume,
                "path": p.path,
                "contentBase64": b64
            });

            match client.post(&url).json(&payload).send().await {
                Ok(resp) => match resp.text().await {
                    Ok(body) => println!("{}", body),
                    Err(e) => {
                        eprintln!("Erreur réponse upload: {}", e);
                        return ExitCode::FAILURE;
                    }
                },
                Err(e) => {
                    eprintln!("Erreur upload fichier: {}", e);
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

            match client.get(&url).send().await {
                Ok(resp) => {
                    if !resp.status().is_success() {
                        eprintln!("Erreur téléchargement HTTP: {}", resp.status());
                        return ExitCode::FAILURE;
                    }
                    match resp.bytes().await {
                        Ok(bytes) => {
                            if let Some(out_path) = g.output {
                                if let Err(e) = fs::write(&out_path, &bytes) {
                                    eprintln!("Erreur écriture dans '{}': {}", out_path.display(), e);
                                    return ExitCode::FAILURE;
                                }
                                println!("Fichier téléchargé avec succès ({} octets) -> {}", bytes.len(), out_path.display());
                            } else {
                                let _ = io::stdout().write_all(&bytes);
                            }
                        }
                        Err(e) => {
                            eprintln!("Erreur lecture flux: {}", e);
                            return ExitCode::FAILURE;
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Erreur requête GET: {}", e);
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
