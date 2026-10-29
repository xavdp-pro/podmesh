//! Native Podman & Docker Volume Plugin (`meshfs`)
//! Exposes `/run/docker/plugins/meshfs.sock` implementing the Docker/Podman VolumeDriver v1.2 HTTP-over-UDS protocol.
//!
//! Enables zero-touch container volume orchestration:
//!   `podman volume create --driver meshfs -o size=50M my-vol`
//!   `podman run --rm -v my-vol:/data alpine sh -c "echo sovereign > /data/hello.txt"`
//!
//! On `Mount`, the volume's files are materialized from the 4 MiB SHA-256 CAS pool into `/var/lib/meshfs/mounts/<name>`.
//! On `Unmount`, all created/modified files in `/var/lib/meshfs/mounts/<name>` are automatically chunked back into
//! `/var/lib/meshfs/chunks/` and sealed into the volume's Merkle tree.

use crate::engine::MeshFSEngine;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tracing::{info, warn};

pub const DEFAULT_PLUGIN_SOCK: &str = "/run/docker/plugins/meshfs.sock";

#[derive(Debug, Default, Deserialize)]
struct PluginReq {
    #[serde(rename = "Name", default)]
    name: String,
    #[serde(rename = "ID", default)]
    _id: String,
    #[serde(rename = "Opts", default)]
    opts: Option<HashMap<String, String>>,
}

fn parse_human_bytes(input: &str) -> Option<u64> {
    let s = input.trim().to_uppercase();
    if s.is_empty() {
        return None;
    }
    if let Some(n) = s.strip_suffix("GIB").or_else(|| s.strip_suffix('G')) {
        return n.trim().parse::<u64>().ok().map(|v| v * 1024 * 1024 * 1024);
    }
    if let Some(n) = s.strip_suffix("MIB").or_else(|| s.strip_suffix('M')) {
        return n.trim().parse::<u64>().ok().map(|v| v * 1024 * 1024);
    }
    if let Some(n) = s.strip_suffix("KIB").or_else(|| s.strip_suffix('K')) {
        return n.trim().parse::<u64>().ok().map(|v| v * 1024);
    }
    s.parse::<u64>().ok()
}

fn sanitize_volume_name(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty()
        || trimmed.contains("..")
        || trimmed.contains('/')
        || trimmed.contains('\\')
    {
        return None;
    }
    Some(trimmed.to_string())
}

fn mountpoint_for(engine: &MeshFSEngine, vol_name: &str) -> PathBuf {
    engine.data_dir.join("mounts").join(vol_name)
}

async fn dispatch_plugin_rpc(
    engine: &MeshFSEngine,
    path: &str,
    body: &[u8],
) -> serde_json::Value {
    let req: PluginReq = serde_json::from_slice(body).unwrap_or_default();

    match path {
        "/Plugin.Activate" => serde_json::json!({
            "Implements": ["VolumeDriver"]
        }),
        "/VolumeDriver.Capabilities" => serde_json::json!({
            "Capabilities": {
                "Scope": "global"
            }
        }),
        "/VolumeDriver.Create" => {
            let Some(vname) = sanitize_volume_name(&req.name) else {
                return serde_json::json!({ "Err": "invalid volume name" });
            };
            if let Err(e) = engine.create_volume(&vname).await {
                return serde_json::json!({ "Err": e.to_string() });
            }
            if let Some(opts) = &req.opts {
                if let Some(sz_str) = opts.get("size").or_else(|| opts.get("max_bytes")) {
                    if let Some(max_b) = parse_human_bytes(sz_str) {
                        let _ = engine.set_volume_quota(&vname, Some(max_b)).await;
                    }
                }
            }
            let mp = mountpoint_for(engine, &vname);
            if let Err(e) = engine.materialize_volume_to_dir(&vname, &mp).await {
                return serde_json::json!({ "Err": e.to_string() });
            }
            serde_json::json!({ "Err": "" })
        }
        "/VolumeDriver.Remove" => {
            let Some(vname) = sanitize_volume_name(&req.name) else {
                return serde_json::json!({ "Err": "invalid volume name" });
            };
            let _ = engine.delete_volume(&vname).await;
            let mp = mountpoint_for(engine, &vname);
            if mp.exists() {
                let _ = fs::remove_dir_all(&mp);
            }
            serde_json::json!({ "Err": "" })
        }
        "/VolumeDriver.Mount" => {
            let Some(vname) = sanitize_volume_name(&req.name) else {
                return serde_json::json!({ "Err": "invalid volume name" });
            };
            let mp = mountpoint_for(engine, &vname);
            if let Err(e) = engine.materialize_volume_to_dir(&vname, &mp).await {
                return serde_json::json!({ "Err": e.to_string() });
            }
            serde_json::json!({
                "Mountpoint": mp.to_string_lossy(),
                "Err": ""
            })
        }
        "/VolumeDriver.Path" => {
            let Some(vname) = sanitize_volume_name(&req.name) else {
                return serde_json::json!({ "Err": "invalid volume name" });
            };
            let mp = mountpoint_for(engine, &vname);
            let _ = fs::create_dir_all(&mp);
            serde_json::json!({
                "Mountpoint": mp.to_string_lossy(),
                "Err": ""
            })
        }
        "/VolumeDriver.Unmount" => {
            let Some(vname) = sanitize_volume_name(&req.name) else {
                return serde_json::json!({ "Err": "invalid volume name" });
            };
            let mp = mountpoint_for(engine, &vname);
            if let Err(e) = engine.sync_dir_to_volume(&vname, &mp).await {
                return serde_json::json!({ "Err": e.to_string() });
            }
            serde_json::json!({ "Err": "" })
        }
        "/VolumeDriver.Get" => {
            let Some(vname) = sanitize_volume_name(&req.name) else {
                return serde_json::json!({ "Err": "invalid volume name" });
            };
            let state = engine.state.read().await;
            if let Some(vol) = state.volumes.get(&vname) {
                let mp = mountpoint_for(engine, &vname);
                let _ = fs::create_dir_all(&mp);
                serde_json::json!({
                    "Volume": {
                        "Name": vol.name,
                        "Mountpoint": mp.to_string_lossy(),
                        "CreatedAt": vol.created_at,
                        "Status": {
                            "merkleRoot": vol.merkle_root,
                            "filesCount": vol.files.len(),
                            "logicalBytes": vol.total_bytes(),
                            "maxBytes": vol.max_bytes.unwrap_or(0)
                        }
                    },
                    "Err": ""
                })
            } else {
                serde_json::json!({ "Err": format!("no such volume: {}", vname) })
            }
        }
        "/VolumeDriver.List" => {
            let state = engine.state.read().await;
            let mut vols = Vec::new();
            for v in state.volumes.values() {
                let mp = mountpoint_for(engine, &v.name);
                vols.push(serde_json::json!({
                    "Name": v.name,
                    "Mountpoint": mp.to_string_lossy()
                }));
            }
            serde_json::json!({
                "Volumes": vols,
                "Err": ""
            })
        }
        _ => serde_json::json!({
            "Err": format!("unknown endpoint: {}", path)
        }),
    }
}

async fn handle_plugin_stream(stream: UnixStream, engine: MeshFSEngine) {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    loop {
        let mut request_line = String::new();
        match reader.read_line(&mut request_line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let trimmed = request_line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        let req_path = parts.get(1).copied().unwrap_or("/");

        let mut content_length = 0usize;
        loop {
            let mut hdr_line = String::new();
            match reader.read_line(&mut hdr_line).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let h = hdr_line.trim();
            if h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                if k.trim().eq_ignore_ascii_case("content-length") {
                    content_length = v.trim().parse().unwrap_or(0);
                }
            }
        }

        let mut body = vec![0u8; content_length.min(4 * 1024 * 1024)];
        if !body.is_empty() && reader.read_exact(&mut body).await.is_err() {
            return;
        }

        let resp_json = dispatch_plugin_rpc(&engine, req_path, &body).await;
        let payload = serde_json::to_vec(&resp_json).unwrap_or_else(|_| b"{}".to_vec());
        let http_resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.docker.plugins.v1.2+json\r\nContent-Length: {}\r\n\r\n",
            payload.len()
        );
        if write_half.write_all(http_resp.as_bytes()).await.is_err() {
            break;
        }
        if write_half.write_all(&payload).await.is_err() {
            break;
        }
        let _ = write_half.flush().await;
    }
}

pub async fn start_docker_volume_plugin(
    engine: MeshFSEngine,
    socket_path: PathBuf,
) -> std::io::Result<()> {
    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if socket_path.exists() {
        let _ = fs::remove_file(&socket_path);
    }
    fs::create_dir_all(engine.data_dir.join("mounts"))?;

    let listener = UnixListener::bind(&socket_path)?;
    // Write `/etc/docker/plugins/meshfs.spec` if `/etc/docker/plugins` can be created
    if fs::create_dir_all("/etc/docker/plugins").is_ok() {
        let _ = fs::write(
            "/etc/docker/plugins/meshfs.spec",
            format!("unix://{}\n", socket_path.display()),
        );
    }

    info!(
        socket = %socket_path.display(),
        "Podman/Docker Volume Plugin (`--driver meshfs`) actif"
    );

    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let eng = engine.clone();
                    tokio::spawn(async move {
                        handle_plugin_stream(stream, eng).await;
                    });
                }
                Err(e) => {
                    warn!(err = %e, "Erreur acceptation socket plugin Podman/Docker");
                }
            }
        }
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_podman_volume_plugin_lifecycle() {
        let tmp_dir =
            std::env::temp_dir().join(format!("meshfs-docker-plugin-test-{}", std::process::id()));
        let sock_path = tmp_dir.join("meshfs.sock");
        let engine = MeshFSEngine::new(&tmp_dir, "test-cluster", 0).await.unwrap();
        start_docker_volume_plugin(engine.clone(), sock_path.clone())
            .await
            .unwrap();

        // 1. Create volume with 10 MiB quota via plugin RPC
        let create_resp = dispatch_plugin_rpc(
            &engine,
            "/VolumeDriver.Create",
            br#"{"Name":"podman-vol","Opts":{"size":"10M"}}"#,
        )
        .await;
        assert_eq!(create_resp["Err"], "");

        // 2. Mount volume via plugin RPC
        let mount_resp = dispatch_plugin_rpc(
            &engine,
            "/VolumeDriver.Mount",
            br#"{"Name":"podman-vol","ID":"container-1"}"#,
        )
        .await;
        assert_eq!(mount_resp["Err"], "");
        let mp = PathBuf::from(mount_resp["Mountpoint"].as_str().unwrap());
        fs::write(mp.join("app.txt"), b"SOVEREIGN_PODMAN_VOLUME_PAYLOAD").unwrap();

        // 3. Unmount volume -> syncs file into 4 MiB CAS pool & updates Merkle root!
        let unmount_resp = dispatch_plugin_rpc(
            &engine,
            "/VolumeDriver.Unmount",
            br#"{"Name":"podman-vol","ID":"container-1"}"#,
        )
        .await;
        assert_eq!(unmount_resp["Err"], "");

        // Verify file is now inside MeshFS CAS!
        let cas_bytes = engine.get_file("podman-vol", "app.txt").await.unwrap();
        assert_eq!(cas_bytes, b"SOVEREIGN_PODMAN_VOLUME_PAYLOAD");

        let _ = fs::remove_dir_all(&tmp_dir);
    }
}
