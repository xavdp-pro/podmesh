//! PMBS — Main Entrypoint and Backup Daemon

mod restore;
mod backup;

use clap::Parser;
use pmbs::cli::{Cli, Commands, DatastoreSubcommand};
use pmbs::control::{query_control_socket, start_control_socket, ControlRequest};
use pmbs::datastore::Datastore;
use pmbs::server::{create_router, AppState, configured_token, api_client};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    let args = Cli::parse();

    let default_level = if args.debug { "debug" } else { "info" };
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("pmbs={},tower_http=info", default_level)));

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(env_filter)
        .with_target(false)
        .with_thread_ids(false)
        .init();

    match args.command {
        Commands::Backup(args) => {
            match backup::run(&args).await {
                Ok(report) => println!("{}", report),
                Err(error) => {
                    eprintln!("{}", serde_json::json!({"ok": false, "error": error,
                        "snapshotPublication": "NOT_CONFIRMED", "unreferencedChunksMayRemain": true}));
                    return ExitCode::FAILURE;
                }
            }
        }
        Commands::Restore(args) => {
            match restore::run(&args).await {
                Ok(report) => println!("{}", report),
                Err(error) => {
                    eprintln!("{}", serde_json::json!({"ok": false, "error": error}));
                    return ExitCode::FAILURE;
                }
            }
        }
        Commands::Daemon(d) => {
            if let Err(e) = configured_token() { eprintln!("{}", e); return ExitCode::FAILURE; }
            info!(
                port = d.port,
                data_dir = %d.data_dir.display(),
                socket = %d.socket.display(),
                "Démarrage du démon PMBS Pure Rust v2.4.1"
            );

            let _ = fs::create_dir_all(&d.data_dir);
            let datastores_dir = d.data_dir.join("datastores");
            let _ = fs::create_dir_all(&datastores_dir);

            let mut map = HashMap::new();
            // Automatically discover or create datastore-default
            if let Ok(entries) = fs::read_dir(&datastores_dir) {
                for entry in entries.flatten() {
                    if entry.path().is_dir() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if let Ok(ds) = Datastore::new(&d.data_dir, &name) {
                            map.insert(name, ds);
                        }
                    }
                }
            }
            if map.is_empty() {
                if let Ok(ds) = Datastore::new(&d.data_dir, "datastore-default") {
                    map.insert("datastore-default".to_string(), ds);
                }
            }

            let app_state = AppState {
                base_dir: d.data_dir.clone(),
                datastores: Arc::new(RwLock::new(map)),
            };

            // Start Unix Domain Socket
            if let Err(e) = start_control_socket(app_state.clone(), d.socket.clone()).await {
                error!(path = %d.socket.display(), "Impossible d'initialiser le socket Unix PMBS: {}", e);
            } else {
                info!(socket = %d.socket.display(), "Socket de contrôle PMBS actif");
            }

            let app = create_router(app_state);
            let addr = std::net::SocketAddr::new(d.bind, d.port);
            let listener = match tokio::net::TcpListener::bind(&addr).await {
                Ok(l) => l,
                Err(e) => {
                    error!("Impossible d'écouter sur {}: {}", addr, e);
                    return ExitCode::FAILURE;
                }
            };

            info!(address = %addr, "Serveur HTTP PMBS actif sur :{} (API /api/v1/*)", d.port);

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

            let client = api_client();
            let url = format!("{}/api/v1/status", p.remote.trim_end_matches('/'));
            match client.get(&url).send().await.and_then(|r| r.error_for_status()) {
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

            let client = api_client();
            let url = format!("{}/api/v1/status", s.remote.trim_end_matches('/'));
            match client.get(&url).send().await.and_then(|r| r.error_for_status()) {
                Ok(resp) => match resp.text().await {
                    Ok(body) => println!("{}", body),
                    Err(e) => {
                        eprintln!("Erreur lecture: {}", e);
                        return ExitCode::FAILURE;
                    }
                },
                Err(e) => {
                    eprintln!("Erreur connexion PMBS: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }

        Commands::Datastore(dc) => match dc.action {
            DatastoreSubcommand::List { remote } => {
                let client = api_client();
                let url = format!("{}/api/v1/datastores", remote.trim_end_matches('/'));
                match client.get(&url).send().await.and_then(|r| r.error_for_status()) {
                    Ok(resp) => match resp.text().await {
                        Ok(body) => println!("{}", body),
                        Err(e) => {
                            eprintln!("Erreur: {}", e);
                            return ExitCode::FAILURE;
                        }
                    },
                    Err(e) => {
                        eprintln!("Erreur datastores: {}", e);
                        return ExitCode::FAILURE;
                    }
                }
            }
            DatastoreSubcommand::Create { name, remote } => {
                let client = api_client();
                let url = format!("{}/api/v1/datastores", remote.trim_end_matches('/'));
                let payload = serde_json::json!({ "name": name });
                match client.post(&url).json(&payload).send().await.and_then(|r| r.error_for_status()) {
                    Ok(resp) => match resp.text().await {
                        Ok(body) => println!("{}", body),
                        Err(e) => {
                            eprintln!("Erreur: {}", e);
                            return ExitCode::FAILURE;
                        }
                    },
                    Err(e) => {
                        eprintln!("Erreur création datastore: {}", e);
                        return ExitCode::FAILURE;
                    }
                }
            }
        },

        Commands::Verify(v) => {
            if let Some(sock) = resolve_socket(v.socket.as_deref()) {
                let req = ControlRequest::Verify {
                    datastore: Some(v.datastore.clone()),
                    snapshot_id: v.snapshot.clone(),
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

            let client = api_client();
            let url = format!(
                "{}/api/v1/datastores/{}/verify",
                v.remote.trim_end_matches('/'),
                v.datastore
            );
            let payload = serde_json::json!({ "snapshotId": v.snapshot });
            match client.post(&url).json(&payload).send().await.and_then(|r| r.error_for_status()) {
                Ok(resp) => match resp.text().await {
                    Ok(body) => println!("{}", body),
                    Err(e) => {
                        eprintln!("Erreur: {}", e);
                        return ExitCode::FAILURE;
                    }
                },
                Err(e) => {
                    eprintln!("Erreur vérification: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }

        Commands::Sync(sc) => {
            let client = api_client();
            let url = format!(
                "{}/api/v1/datastores/{}/sync",
                sc.remote.trim_end_matches('/'),
                sc.target_datastore
            );
            let payload = serde_json::json!({
                "remoteUrl": sc.remote_url,
                "remoteDatastore": sc.remote_datastore
            });

            match client.post(&url).json(&payload).send().await.and_then(|r| r.error_for_status()) {
                Ok(resp) => match resp.text().await {
                    Ok(body) => println!("{}", body),
                    Err(e) => {
                        eprintln!("Erreur réponse sync: {}", e);
                        return ExitCode::FAILURE;
                    }
                },
                Err(e) => {
                    eprintln!("Erreur requête sync: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
    }

    ExitCode::SUCCESS
}

fn resolve_socket(custom: Option<&Path>) -> Option<std::path::PathBuf> {
    if let Some(p) = custom {
        return Some(p.to_path_buf());
    }
    for candidate in ["/run/shaper/pmbs.sock", "/var/lib/pmbs/pmbs.sock"] {
        let p = Path::new(candidate);
        if p.exists() {
            return Some(p.to_path_buf());
        }
    }
    None
}
