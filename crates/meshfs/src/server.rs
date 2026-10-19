//! MeshFS REST API Server (Axum)
//! Provides 100% compatibility with Proxmox VE Storage Plugin (MeshFSPlugin.pm)
//! and PodMesh Distributed Orchestrator.

use crate::engine::MeshFSEngine;
use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tower_http::cors::{Any, CorsLayer};

/// Query parameters for downloading files.
#[derive(Debug, Deserialize)]
pub struct DownloadQuery {
    #[serde(default = "default_volume")]
    pub volume: String,
    pub path: String,
}

fn default_volume() -> String {
    "default".to_string()
}

/// Request body for joining a cluster node.
#[derive(Debug, Deserialize)]
pub struct JoinRequest {
    #[serde(rename = "targetAddress", alias = "address")]
    pub target_address: String,
    #[serde(default)]
    pub token: String,
}

/// Request body for creating a volume.
#[derive(Debug, Deserialize)]
pub struct CreateVolumeRequest {
    pub name: String,
}

/// Request body for snapshot operations.
#[derive(Debug, Deserialize)]
pub struct SnapshotRequest {
    #[serde(default = "default_volume")]
    pub volume: String,
    #[serde(default)]
    pub name: Option<String>,
}

/// Request body for snapshot deletion.
#[derive(Debug, Deserialize)]
pub struct DeleteSnapshotRequest {
    #[serde(default = "default_volume")]
    pub volume: String,
    pub name: String,
}

/// Request body for file uploads (Base64 payload).
#[derive(Debug, Deserialize)]
pub struct UploadRequest {
    #[serde(default = "default_volume")]
    pub volume: String,
    pub path: String,
    #[serde(rename = "contentBase64")]
    pub content_base64: String,
}

/// Builds the Axum Router with all compatible endpoints.
pub fn create_router(engine: MeshFSEngine) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        // Status routes
        .route("/", get(status_handler))
        .route("/status", get(status_handler))
        .route("/api/v1/meshfs/status", get(status_handler))
        // Nodes routes
        .route("/nodes", get(nodes_handler))
        .route("/api/v1/meshfs/nodes", get(nodes_handler))
        .route("/nodes/join", post(join_handler))
        .route("/api/v1/meshfs/nodes/join", post(join_handler))
        // Volumes routes
        .route("/volumes", get(volumes_handler).post(create_volume_handler))
        .route(
            "/api/v1/meshfs/volumes",
            get(volumes_handler).post(create_volume_handler),
        )
        // Snapshots routes
        .route(
            "/snapshots",
            get(snapshots_handler).post(create_snapshot_handler),
        )
        .route(
            "/api/v1/meshfs/snapshots",
            get(snapshots_handler).post(create_snapshot_handler),
        )
        .route("/snapshots/delete", post(delete_snapshot_handler))
        .route("/api/v1/meshfs/snapshots/delete", post(delete_snapshot_handler))
        // Files download / upload routes
        .route("/files/download", get(download_file_handler))
        .route("/api/v1/meshfs/files/download", get(download_file_handler))
        .route("/files/upload", post(upload_file_handler))
        .route("/api/v1/meshfs/files/upload", post(upload_file_handler))
        // Diagnostics & Integrity audit routes
        .route("/diagnostics", get(diagnostics_handler))
        .route("/api/v1/meshfs/diagnostics", get(diagnostics_handler))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(cors)
        .with_state(engine)
}

// Handler implementations

async fn status_handler(State(engine): State<MeshFSEngine>) -> impl IntoResponse {
    let report = engine.get_status().await;
    Json(report)
}

async fn nodes_handler(State(engine): State<MeshFSEngine>) -> impl IntoResponse {
    let state = engine.state.read().await;
    #[derive(Serialize)]
    struct NodesResp {
        ok: bool,
        nodes: Vec<crate::engine::NodeInfo>,
    }
    Json(NodesResp {
        ok: true,
        nodes: state.nodes.clone(),
    })
}

async fn join_handler(
    State(engine): State<MeshFSEngine>,
    Json(req): Json<JoinRequest>,
) -> Response {
    if req.target_address.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": "targetAddress requis" })),
        )
            .into_response();
    }

    match engine.join_cluster(&req.target_address, &req.token).await {
        Ok(nodes) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": true,
                "connectedTo": req.target_address,
                "nodes": nodes
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": e })),
        )
            .into_response(),
    }
}

async fn volumes_handler(State(engine): State<MeshFSEngine>) -> impl IntoResponse {
    let volumes = engine.list_volumes().await;
    Json(serde_json::json!({
        "ok": true,
        "volumes": volumes
    }))
}

async fn create_volume_handler(
    State(engine): State<MeshFSEngine>,
    Json(req): Json<CreateVolumeRequest>,
) -> Response {
    let name = req.name.trim();
    if name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": "name requis" })),
        )
            .into_response();
    }

    match engine.create_volume(name).await {
        Ok(vol) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "ok": true, "volume": vol })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn snapshots_handler(
    State(engine): State<MeshFSEngine>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let vol_filter = params.get("volume").map(|s| s.as_str());
    let snapshots = engine.list_snapshots(vol_filter).await;
    Json(serde_json::json!({
        "ok": true,
        "snapshots": snapshots
    }))
}

async fn create_snapshot_handler(
    State(engine): State<MeshFSEngine>,
    Json(req): Json<SnapshotRequest>,
) -> Response {
    let vol_name = if req.volume.trim().is_empty() {
        "default"
    } else {
        req.volume.trim()
    };

    match engine
        .create_snapshot(vol_name, req.name.as_deref())
        .await
    {
        Ok(snap) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "ok": true, "snapshot": snap })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn delete_snapshot_handler(
    State(engine): State<MeshFSEngine>,
    Json(req): Json<DeleteSnapshotRequest>,
) -> Response {
    let vol_name = if req.volume.trim().is_empty() {
        "default"
    } else {
        req.volume.trim()
    };
    let snap_name = req.name.trim();
    if snap_name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": "name requis" })),
        )
            .into_response();
    }

    match engine.delete_snapshot(vol_name, snap_name).await {
        Ok(true) => (
            StatusCode::OK,
            Json(serde_json::json!({ "ok": true, "deleted": snap_name })),
        )
            .into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "ok": false, "error": "Snapshot introuvable" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn download_file_handler(
    State(engine): State<MeshFSEngine>,
    Query(query): Query<DownloadQuery>,
) -> Response {
    match engine.get_file(&query.volume, &query.path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/octet-stream"),
                (
                    header::CONTENT_DISPOSITION,
                    &format!("attachment; filename=\"{}\"", query.path),
                ),
            ],
            Body::from(bytes),
        )
            .into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn upload_file_handler(
    State(engine): State<MeshFSEngine>,
    Json(req): Json<UploadRequest>,
) -> Response {
    if req.path.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": "path requis" })),
        )
            .into_response();
    }

    let decoded = match hex_or_base64_decode(&req.content_base64) {
        Ok(data) => data,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "ok": false, "error": format!("Base64 invalide: {}", e) })),
            )
                .into_response()
        }
    };

    match engine.put_file(&req.volume, &req.path, &decoded).await {
        Ok(meta) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "ok": true, "file": meta })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn diagnostics_handler(
    State(engine): State<MeshFSEngine>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let vol_filter = params.get("volume").map(|s| s.as_str());
    let deep = params.get("deep").map(|s| s == "true" || s == "1").unwrap_or(true);
    let report = engine.audit_integrity(vol_filter, deep).await;
    Json(report)
}

/// Fallback base64 decoder using simple standard translation table.
fn hex_or_base64_decode(input: &str) -> Result<Vec<u8>, String> {
    // Simple fast base64 decoder without extra external dependencies
    let clean: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    let chars: Vec<char> = clean.chars().collect();
    let mut out = Vec::new();

    let b64_val = |c: char| -> Result<u8, String> {
        match c {
            'A'..='Z' => Ok(c as u8 - b'A'),
            'a'..='z' => Ok(c as u8 - b'a' + 26),
            '0'..='9' => Ok(c as u8 - b'0' + 52),
            '+' => Ok(62),
            '/' => Ok(63),
            '=' => Ok(0),
            _ => Err(format!("Caractère base64 invalide: {}", c)),
        }
    };

    let mut i = 0;
    while i < chars.len() {
        if i + 4 > chars.len() {
            break;
        }
        let b0 = b64_val(chars[i])?;
        let b1 = b64_val(chars[i + 1])?;
        let b2 = b64_val(chars[i + 2])?;
        let b3 = b64_val(chars[i + 3])?;

        let trip = ((b0 as u32) << 18) | ((b1 as u32) << 12) | ((b2 as u32) << 6) | (b3 as u32);

        out.push(((trip >> 16) & 0xFF) as u8);
        if chars[i + 2] != '=' {
            out.push(((trip >> 8) & 0xFF) as u8);
        }
        if chars[i + 3] != '=' {
            out.push((trip & 0xFF) as u8);
        }

        i += 4;
    }

    Ok(out)
}
