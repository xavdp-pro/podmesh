//! MeshFS REST API Server (Axum)
//! Provides 100% compatibility with Proxmox VE Storage Plugin (MeshFSPlugin.pm)
//! and PodMesh Distributed Orchestrator.

use crate::engine::MeshFSEngine;
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Query, State},
    http::{header, HeaderMap, StatusCode},
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
        .route("/internal/append", post(raft_append))
        .route("/internal/vote", post(raft_vote))
        .route("/internal/snapshot", post(raft_snapshot))
        .route("/internal/write", post(raft_write))
        .route("/internal/read", post(raft_read))
        .route("/internal/namespace", post(raft_namespace))
        .route("/internal/health", post(raft_health))
        .route("/internal/inode", post(raft_inode))
        .route("/internal/chunk", post(raft_chunk))
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
        .route(
            "/api/v1/meshfs/snapshots/delete",
            post(delete_snapshot_handler),
        )
        // Bounded experimental live file access.
        .route("/api/v1/meshfs/files/inode", get(live_inode_handler))
        .route("/api/v1/meshfs/files/range", get(live_range_handler))
        .route("/api/v1/meshfs/files/replay-epoch", get(live_replay_epoch_handler))
        .route("/api/v1/meshfs/files/live", post(live_mutation_handler))
        // Files download / upload routes
        .route("/files/download", get(download_file_handler))
        .route("/api/v1/meshfs/files/download", get(download_file_handler))
        .route(
            "/files/stream",
            post(stream_upload_handler).layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/api/v1/meshfs/files/stream",
            post(stream_upload_handler).layer(DefaultBodyLimit::disable()),
        )
        .route("/files/upload", post(upload_file_handler))
        .route("/api/v1/meshfs/files/upload", post(upload_file_handler))
        // Diagnostics & Integrity audit routes
        .route("/diagnostics", get(diagnostics_handler))
        .route("/api/v1/meshfs/diagnostics", get(diagnostics_handler))
        .route("/metrics", get(metrics_handler))
        .route("/cas/chunk/:hash", get(cas_chunk_handler))
        .route("/api/v1/meshfs/cas/chunk/:hash", get(cas_chunk_handler))
        .route("/api/v1/meshfs/gc", post(gc_handler))
        .route("/api/v1/meshfs/volumes/quota", post(volume_quota_handler))
        .layer(DefaultBodyLimit::max(128 * 1024 * 1024))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(cors)
        .with_state(engine)
}

// Handler implementations

async fn status_handler(State(engine): State<MeshFSEngine>) -> impl IntoResponse {
    let report = engine.get_status().await;
    Json(report)
}

async fn metrics_handler(State(engine): State<MeshFSEngine>) -> impl IntoResponse {
    let st = engine.get_status().await;
    let body = format!(
        "# HELP meshfs_volumes_total Number of logical MeshFS volumes\n\
         # TYPE meshfs_volumes_total gauge\n\
         meshfs_volumes_total {}\n\
         # HELP meshfs_snapshots_total Number of CoW Merkle snapshots\n\
         # TYPE meshfs_snapshots_total gauge\n\
         meshfs_snapshots_total {}\n\
         # HELP meshfs_files_total Total files across all volumes\n\
         # TYPE meshfs_files_total gauge\n\
         meshfs_files_total {}\n\
         # HELP meshfs_logical_bytes_total Total logical bytes stored across volumes\n\
         # TYPE meshfs_logical_bytes_total gauge\n\
         meshfs_logical_bytes_total {}\n\
         # HELP meshfs_physical_cas_chunks_total Total physical 4 MiB CAS chunks on disk\n\
         # TYPE meshfs_physical_cas_chunks_total gauge\n\
         meshfs_physical_cas_chunks_total {}\n\
         # HELP meshfs_physical_cas_bytes_total Total physical CAS bytes on disk\n\
         # TYPE meshfs_physical_cas_bytes_total gauge\n\
         meshfs_physical_cas_bytes_total {}\n\
         # HELP meshfs_dedup_saved_bytes_total Bytes saved by CAS deduplication\n\
         # TYPE meshfs_dedup_saved_bytes_total gauge\n\
         meshfs_dedup_saved_bytes_total {}\n\
         # HELP meshfs_dedup_ratio Deduplication ratio (logical / physical)\n\
         # TYPE meshfs_dedup_ratio gauge\n\
         meshfs_dedup_ratio {:.4}\n\
         # HELP meshfs_self_healed_chunks_total Total CAS chunks automatically repaired via P2P self-healing\n\
         # TYPE meshfs_self_healed_chunks_total counter\n\
         meshfs_self_healed_chunks_total {}\n",
        st.volumes_count,
        st.snapshots_count,
        st.total_files,
        st.total_logical_bytes,
        st.physical_cas_chunks,
        st.physical_cas_bytes,
        st.dedup_saved_bytes,
        st.dedup_ratio,
        st.self_healed_chunks,
    );
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
        body,
    )
}

async fn cas_chunk_handler(
    State(engine): State<MeshFSEngine>,
    axum::extract::Path(hash): axum::extract::Path<String>,
) -> Response {
    let clean = crate::chunk::ChunkStore::normalize_id(&hash).to_string();
    if clean.len() != 64
        || !clean
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return (StatusCode::BAD_REQUEST, "Invalid chunk hash").into_response();
    }
    let target = engine.chunk_store.chunk_path(&clean);
    match std::fs::read(&target) {
        Ok(bytes)
            if bytes.len() <= crate::chunk::CHUNK_SIZE
                && crate::chunk::verify_cas_digest(&bytes, &clean) =>
        {
            (
                StatusCode::OK,
                [
                    ("content-type", "application/octet-stream"),
                    ("x-podmesh-cas-blake3", clean.as_str()),
                    ("x-podmesh-cas-sha256", clean.as_str()),
                ],
                bytes,
            )
                .into_response()
        }
        _ => (StatusCode::NOT_FOUND, "Chunk not found or corrupted").into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct GcHttpRequest {
    #[serde(default)]
    grace_seconds: u64,
    #[serde(default)]
    dry_run: bool,
}

async fn gc_handler(
    State(engine): State<MeshFSEngine>,
    Json(req): Json<GcHttpRequest>,
) -> impl IntoResponse {
    let rep = engine
        .run_universal_cas_gc(req.grace_seconds, req.dry_run)
        .await;
    Json(rep)
}

#[derive(Debug, Deserialize)]
struct QuotaHttpRequest {
    volume: String,
    max_bytes: Option<u64>,
}

async fn volume_quota_handler(
    State(engine): State<MeshFSEngine>,
    Json(req): Json<QuotaHttpRequest>,
) -> Response {
    match engine.set_volume_quota(&req.volume, req.max_bytes).await {
        Ok(v) => Json(serde_json::json!({
            "status": "ok",
            "volume": v.name,
            "logical_used_bytes": v.total_bytes(),
            "max_bytes": v.max_bytes.unwrap_or(0),
        }))
        .into_response(),
        Err(e) => unavailable(e),
    }
}

async fn nodes_handler(State(engine): State<MeshFSEngine>) -> impl IntoResponse {
    let nodes = if let Some(c) = engine.cluster.get() {
        c.nodes().await
    } else {
        engine.state.read().await.nodes.clone()
    };
    #[derive(Serialize)]
    struct NodesResp {
        ok: bool,
        nodes: Vec<crate::engine::NodeInfo>,
    }
    Json(NodesResp { ok: true, nodes })
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
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "ok": false, "error": e })),
        )
            .into_response(),
    }
}

async fn volumes_handler(State(engine): State<MeshFSEngine>) -> Response {
    match engine.list_volumes().await {
        Ok(volumes) => Json(serde_json::json!({"ok":true,"volumes":volumes})).into_response(),
        Err(e) => unavailable(e),
    }
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
            if e.to_string().contains("live file requires precondition") {
                StatusCode::CONFLICT
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            },
            Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn snapshots_handler(
    State(engine): State<MeshFSEngine>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let vol_filter = params.get("volume").map(|s| s.as_str());
    match engine.list_snapshots(vol_filter).await {
        Ok(snapshots) => Json(serde_json::json!({"ok":true,"snapshots":snapshots})).into_response(),
        Err(e) => unavailable(e),
    }
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

    match engine.create_snapshot(vol_name, req.name.as_deref()).await {
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
            if e.to_string().contains("live file requires precondition") {
                StatusCode::CONFLICT
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            },
            Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn download_file_handler(
    State(engine): State<MeshFSEngine>,
    Query(query): Query<DownloadQuery>,
) -> Response {
    let inode = match engine.file_inode(&query.volume, &query.path).await {
        Ok(i) => i,
        Err(e) => {
            return (
                if e.kind() == std::io::ErrorKind::NotFound {
                    StatusCode::NOT_FOUND
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                },
                Json(serde_json::json!({"error":e.to_string()})),
            )
                .into_response()
        }
    };
    if inode.digest.starts_with("merkle-blake3:") || inode.digest.starts_with("merkle-sha256:") {
        return (
            StatusCode::CONFLICT,
            "live files require the range interface or mount; digest is a manifest hash",
        )
            .into_response();
    }
    let size = inode.size;
    let digest = inode.digest.clone();
    let stream = futures_util::stream::try_unfold(
        (engine, inode, 0usize, 0u64, blake3::Hasher::new()),
        |(engine, inode, index, offset, mut hash)| async move {
            if index == inode.chunks.len() {
                if offset != inode.size
                    || format!("blake3:{}", hash.finalize().to_hex()) != inode.digest
                {
                    return Err(std::io::Error::other("file digest mismatch"));
                }
                return Ok(None);
            }
            let c = &inode.chunks[index];
            let bytes = engine.file_chunk(&c.id).await?;
            if c.offset != offset || bytes.len() != c.size {
                return Err(std::io::Error::other("chunk layout mismatch"));
            }
            hash.update(&bytes);
            let offset = offset + bytes.len() as u64;
            if index + 1 == inode.chunks.len()
                && (offset != inode.size
                    || format!("blake3:{}", hash.clone().finalize().to_hex()) != inode.digest)
            {
                return Err(std::io::Error::other("file digest mismatch"));
            }
            Ok(Some((
                bytes::Bytes::from(bytes),
                (engine, inode, index + 1, offset, hash),
            )))
        },
    );
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (header::CONTENT_LENGTH, size.to_string()),
            (header::ETAG, format!("\"{}\"", digest)),
        ],
        Body::from_stream(stream),
    )
        .into_response()
}
async fn stream_upload_handler(
    State(engine): State<MeshFSEngine>,
    Query(query): Query<DownloadQuery>,
    body: Body,
) -> Response {
    use futures_util::TryStreamExt;
    if query.path.trim().is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let stream = body.into_data_stream().map_err(std::io::Error::other);
    let reader = tokio_util::io::StreamReader::new(stream);
    match engine.put_stream(&query.volume, &query.path, reader).await {
        Ok(inode) => (
            StatusCode::CREATED,
            Json(serde_json::json!({"ok":true,"file":inode})),
        )
            .into_response(),
        Err(e) => unavailable(e),
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

    let decoded =
        match hex_or_base64_decode(&req.content_base64) {
            Ok(data) => data,
            Err(e) => return (
                StatusCode::BAD_REQUEST,
                Json(
                    serde_json::json!({ "ok": false, "error": format!("Base64 invalide: {}", e) }),
                ),
            )
                .into_response(),
        };

    if engine.cluster.get().is_some() && decoded.len() > crate::cluster::MAX_INLINE_BYTES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            "clustered file limit is 16 MiB",
        )
            .into_response();
    }
    match engine.put_file(&req.volume, &req.path, &decoded).await {
        Ok(meta) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "ok": true, "file": meta })),
        )
            .into_response(),
        Err(e) => (
            if e.to_string().contains("live file requires precondition") {
                StatusCode::CONFLICT
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            },
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
    let deep = params
        .get("deep")
        .map(|s| s == "true" || s == "1")
        .unwrap_or(true);
    let report = engine.audit_integrity(vol_filter, deep).await;
    Json(report)
}

fn hex_or_base64_decode(input: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(input)
        .map_err(|e| e.to_string())
}

fn unavailable(e: impl std::fmt::Display) -> Response {
    (
        if e.to_string().contains("live compare conflict")
            || e.to_string().contains("live file requires precondition")
            || e.to_string().contains("live replay epoch expired")
        {
            StatusCode::CONFLICT
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(serde_json::json!({"ok":false,"error":e.to_string()})),
    )
        .into_response()
}
fn internal_cluster(
    engine: &MeshFSEngine,
    headers: &HeaderMap,
) -> Result<std::sync::Arc<crate::cluster::Cluster>, Response> {
    match engine.cluster.get() {
        Some(c) if c.authorized(headers) => Ok(c.clone()),
        _ => Err(StatusCode::UNAUTHORIZED.into_response()),
    }
}
async fn raft_append(
    State(e): State<MeshFSEngine>,
    h: HeaderMap,
    Json(r): Json<openraft::raft::AppendEntriesRequest<crate::cluster::Types>>,
) -> Response {
    let c = match internal_cluster(&e, &h) {
        Ok(c) => c,
        Err(r) => return r,
    };
    Json(c.raft.append_entries(r).await).into_response()
}
async fn raft_vote(
    State(e): State<MeshFSEngine>,
    h: HeaderMap,
    Json(r): Json<openraft::raft::VoteRequest<u64>>,
) -> Response {
    let c = match internal_cluster(&e, &h) {
        Ok(c) => c,
        Err(r) => return r,
    };
    Json(c.raft.vote(r).await).into_response()
}
async fn raft_snapshot(
    State(e): State<MeshFSEngine>,
    h: HeaderMap,
    Json(r): Json<openraft::raft::InstallSnapshotRequest<crate::cluster::Types>>,
) -> Response {
    let c = match internal_cluster(&e, &h) {
        Ok(c) => c,
        Err(r) => return r,
    };
    Json(c.raft.install_snapshot(r).await).into_response()
}
async fn raft_write(
    State(e): State<MeshFSEngine>,
    h: HeaderMap,
    Json(r): Json<crate::cluster::Command>,
) -> Response {
    let c = match internal_cluster(&e, &h) {
        Ok(c) => c,
        Err(r) => return r,
    };
    Json(c.local_write(r).await).into_response()
}
async fn raft_read(
    State(e): State<MeshFSEngine>,
    h: HeaderMap,
    Json(r): Json<crate::cluster::ReadRequest>,
) -> Response {
    let c = match internal_cluster(&e, &h) {
        Ok(c) => c,
        Err(r) => return r,
    };
    Json(c.local_read(r).await).into_response()
}
async fn raft_namespace(
    State(e): State<MeshFSEngine>,
    h: HeaderMap,
    Json(_): Json<()>,
) -> Response {
    let c = match internal_cluster(&e, &h) {
        Ok(c) => c,
        Err(r) => return r,
    };
    Json(c.local_namespace().await).into_response()
}
async fn raft_health(State(e): State<MeshFSEngine>, h: HeaderMap, Json(_): Json<()>) -> Response {
    let c = match internal_cluster(&e, &h) {
        Ok(c) => c,
        Err(r) => return r,
    };
    Json(Ok::<_, String>(c.metrics())).into_response()
}

async fn raft_inode(
    State(e): State<MeshFSEngine>,
    h: HeaderMap,
    Json(r): Json<crate::cluster::ReadRequest>,
) -> Response {
    let c = match internal_cluster(&e, &h) {
        Ok(c) => c,
        Err(r) => return r,
    };
    Json(c.local_inode(r).await).into_response()
}
async fn raft_chunk(
    State(e): State<MeshFSEngine>,
    h: HeaderMap,
    Json(id): Json<String>,
) -> Response {
    let c = match internal_cluster(&e, &h) {
        Ok(c) => c,
        Err(r) => return r,
    };
    Json(c.local_chunk(&id)).into_response()
}

async fn live_inode_handler(
    State(e): State<MeshFSEngine>,
    Query(q): Query<DownloadQuery>,
) -> Response {
    match e.file_inode(&q.volume, &q.path).await {
        Ok(i) => Json(i).into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => StatusCode::NOT_FOUND.into_response(),
        Err(e) => unavailable(e),
    }
}
#[derive(Deserialize)]
struct RangeQuery {
    volume: String,
    path: String,
    offset: u64,
    length: usize,
}
async fn live_range_handler(
    State(e): State<MeshFSEngine>,
    Query(q): Query<RangeQuery>,
) -> Response {
    if q.length > crate::live::MAX_IO {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match e.live_read(&q.volume, &q.path, q.offset, q.length).await {
        Ok(b) => b.into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => StatusCode::NOT_FOUND.into_response(),
        Err(e) => unavailable(e),
    }
}
async fn live_replay_epoch_handler(State(e): State<MeshFSEngine>) -> Response {
    match e.live_replay_epoch().await {
        Ok(v) => Json(v).into_response(),
        Err(e) => unavailable(e),
    }
}
async fn live_mutation_handler(
    State(e): State<MeshFSEngine>,
    Json(r): Json<crate::live::Request>,
) -> Response {
    if r.epoch.is_none() || r.base.is_none()
        || (matches!(r.operation, crate::live::Operation::Rename { .. }) && r.destination.is_none())
    {
        return (
            StatusCode::PRECONDITION_REQUIRED,
            "explicit live preconditions required",
        )
            .into_response();
    }
    match e.live_apply(r).await {
        Ok(v) => Json(v).into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => StatusCode::NOT_FOUND.into_response(),
        Err(e) => unavailable(e),
    }
}
