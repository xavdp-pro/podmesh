//! PMBS REST API Server (Axum)
//! Exposes backup endpoints on standard port 8007.

use crate::datastore::{BackupManifest, Datastore, CHUNK_SIZE, validate_name, validate_chunk_id};
use crate::verify::verify_snapshot;
use axum::{
    body::Body,
    middleware::{self, Next},
    extract::Request,
    extract::{DefaultBodyLimit, Path as AxPath, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;


#[derive(Clone)]
pub struct AppState {
    pub base_dir: PathBuf,
    pub datastores: Arc<RwLock<HashMap<String, Datastore>>>,
}

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub ok: bool,
    pub service: String,
    pub version: String,
    pub datastores: Vec<String>,
    #[serde(rename = "dedupEngine")]
    pub dedup_engine: String,
    #[serde(rename = "op10VerifierReady")]
    pub op10_verifier_ready: bool,
    #[serde(rename = "remoteSyncEnabled")]
    pub remote_sync_enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct CreateDatastoreRequest {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct SyncRequest {
    #[serde(rename = "remoteUrl")]
    pub remote_url: String,
    #[serde(rename = "remoteDatastore")]
    pub remote_datastore: String,
}

#[derive(Debug, Deserialize)]
pub struct VerifyRequest {
    #[serde(rename = "snapshotId")]
    pub snapshot_id: Option<String>,
}

pub fn create_router(state: AppState) -> Router {

    Router::new()
        .route("/", get(status_handler))
        .route("/status", get(status_handler))
        .route("/api/v1/status", get(status_handler))
        .route(
            "/api/v1/datastores",
            get(list_datastores_handler).post(create_datastore_handler),
        )
        .route(
            "/api/v1/datastores/:datastore/snapshots",
            get(list_snapshots_handler).post(save_snapshot_handler),
        )
        .route(
            "/api/v1/datastores/:datastore/chunks/:chunk_id",
            get(get_chunk_handler),
        )
        .route(
            "/api/v1/datastores/:datastore/chunks",
            post(upload_chunk_handler).layer(DefaultBodyLimit::max(CHUNK_SIZE)),
        )
        .route(
            "/api/v1/datastores/:datastore/verify",
            post(verify_datastore_handler),
        )
        .route(
            "/api/v1/datastores/:datastore/sync",
            post(sync_datastore_handler),
        )
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(middleware::from_fn(authorize))
        .with_state(state)
}

/// Required for HTTP on every route; local IPC uses owner-only filesystem permissions.
pub fn configured_token() -> Result<String, String> {
    let token = std::env::var("PMBS_API_TOKEN").map_err(|_| "PMBS_API_TOKEN is required")?;
    if token.len() < 32 || token.len() > 256 || !token.bytes().all(|b| (33..=126).contains(&b)) {
        return Err("PMBS_API_TOKEN must be 32..256 printable non-space bytes".into());
    }
    Ok(token)
}
async fn authorize(request: Request, next: Next) -> Response {
    let expected = match configured_token() { Ok(v) => v, Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response() };
    let supplied = request.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).unwrap_or("");
    if supplied.len() != expected.len() || supplied.bytes().zip(expected.bytes()).fold(0u8, |diff, (a,b)| diff | (a ^ b)) != 0 {
        return (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, "Bearer")], "authentication required").into_response();
    }
    next.run(request).await
}
pub fn api_client() -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    if let Ok(token) = configured_token() {
        if let Ok(mut v) = reqwest::header::HeaderValue::from_str(&format!("Bearer {}", token)) { v.set_sensitive(true); headers.insert(reqwest::header::AUTHORIZATION, v); }
    }
    reqwest::Client::builder().default_headers(headers).redirect(reqwest::redirect::Policy::none()).build().expect("HTTP client construction")
}

// Handlers

async fn status_handler(State(state): State<AppState>) -> impl IntoResponse {
    let stores = state.datastores.read().await;
    let names: Vec<String> = stores.keys().cloned().collect();
    Json(StatusResponse {
        ok: true,
        service: "podmesh-backup-server".to_string(),
        version: "2.4.1".to_string(),
        datastores: names,
        dedup_engine: "BLAKE3 CAS 4MB (Pure Rust)".to_string(),
        op10_verifier_ready: false,
        remote_sync_enabled: false,
    })
}

async fn list_datastores_handler(State(state): State<AppState>) -> impl IntoResponse {
    let stores = state.datastores.read().await;
    let list: Vec<String> = stores.keys().cloned().collect();
    Json(serde_json::json!({ "ok": true, "datastores": list }))
}

async fn create_datastore_handler(
    State(state): State<AppState>,
    Json(req): Json<CreateDatastoreRequest>,
) -> Response {
    let name = req.name.as_str();
    if validate_name(name).is_err() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": "invalid datastore name" })),
        )
            .into_response();
    }

    let mut stores = state.datastores.write().await;
    if stores.contains_key(name) {
        return (
            StatusCode::OK,
            Json(serde_json::json!({ "ok": true, "datastore": name, "created": false })),
        )
            .into_response();
    }

    match Datastore::new(&state.base_dir, name) {
        Ok(ds) => {
            stores.insert(name.to_string(), ds);
            (
                StatusCode::CREATED,
                Json(serde_json::json!({ "ok": true, "datastore": name, "created": true })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn list_snapshots_handler(
    State(state): State<AppState>,
    AxPath(datastore): AxPath<String>,
) -> Response {
    let stores = state.datastores.read().await;
    let ds = match stores.get(&datastore) {
        Some(d) => d,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "ok": false, "error": "Datastore introuvable" })),
            )
                .into_response()
        }
    };

    let snaps = ds.list_snapshots();
    Json(serde_json::json!({ "ok": true, "snapshots": snaps })).into_response()
}

async fn save_snapshot_handler(
    State(state): State<AppState>,
    AxPath(datastore): AxPath<String>,
    headers: HeaderMap,
    Json(manifest): Json<BackupManifest>,
) -> Response {
    let stores = state.datastores.read().await;
    let ds = match stores.get(&datastore) {
        Some(d) => d,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "ok": false, "error": "Datastore introuvable" })),
            )
                .into_response()
        }
    };

    if headers.get_all(header::IF_NONE_MATCH).iter().count() > 1 {
        return (StatusCode::BAD_REQUEST, "ambiguous If-None-Match").into_response();
    }
    let create_only = match headers.get(header::IF_NONE_MATCH) {
        None => false,
        Some(v) if v.as_bytes() == b"*" => true,
        Some(_) => return (StatusCode::BAD_REQUEST, "only If-None-Match: * is supported").into_response(),
    };
    let publication = if create_only { ds.create_snapshot(&manifest) } else { ds.save_snapshot(&manifest) };
    match publication {
        Ok(_) => (
            StatusCode::CREATED,
            Json(if create_only { serde_json::json!({ "ok": true, "snapshotId": manifest.id, "createdOnly": true }) }
                 else { serde_json::json!({ "ok": true, "snapshotId": manifest.id }) }),
        )
            .into_response(),
        Err(e) => (
            match e.kind() { std::io::ErrorKind::InvalidInput => StatusCode::BAD_REQUEST, std::io::ErrorKind::AlreadyExists => if create_only { StatusCode::PRECONDITION_FAILED } else { StatusCode::CONFLICT }, _ => StatusCode::INTERNAL_SERVER_ERROR },
            Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn get_chunk_handler(
    State(state): State<AppState>,
    AxPath((datastore, chunk_id)): AxPath<(String, String)>,
) -> Response {
    if validate_chunk_id(&chunk_id).is_err() { return (StatusCode::BAD_REQUEST, "invalid chunk id").into_response(); }
    let stores = state.datastores.read().await;
    let ds = match stores.get(&datastore) {
        Some(d) => d,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "ok": false, "error": "Datastore introuvable" })),
            )
                .into_response()
        }
    };

    match ds.read_chunk(&chunk_id) {
        Ok(bytes) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/octet-stream")],
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

async fn upload_chunk_handler(
    State(state): State<AppState>,
    AxPath(datastore): AxPath<String>,
    body: axum::body::Bytes,
) -> Response {
    let stores = state.datastores.read().await;
    let ds = match stores.get(&datastore) {
        Some(d) => d,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "ok": false, "error": "Datastore introuvable" })),
            )
                .into_response()
        }
    };

    match ds.write_chunk(&body) {
        Ok(chunk_id) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "ok": true, "chunkId": chunk_id })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn verify_datastore_handler(
    State(state): State<AppState>,
    AxPath(datastore): AxPath<String>,
    Json(req): Json<VerifyRequest>,
) -> Response {
    let stores = state.datastores.read().await;
    let ds = match stores.get(&datastore) {
        Some(d) => d,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "ok": false, "error": "Datastore introuvable" })),
            )
                .into_response()
        }
    };

    let snapshots = ds.list_snapshots();
    let mut reports = Vec::new();

    for snap in &snapshots {
        if let Some(ref target_id) = req.snapshot_id {
            if &snap.id != target_id {
                continue;
            }
        }
        let report = verify_snapshot(ds, snap);
        reports.push(report);
    }

    Json(serde_json::json!({
        "ok": true,
        "datastore": datastore,
        "reports": reports
    }))
    .into_response()
}

async fn sync_datastore_handler(
    State(_state): State<AppState>, AxPath(_datastore): AxPath<String>, Json(_req): Json<SyncRequest>,
) -> Response {
    (StatusCode::FORBIDDEN, "remote sync disabled: configure an outbound destination policy before enabling").into_response()
}
