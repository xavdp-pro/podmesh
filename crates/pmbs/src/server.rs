//! PMBS REST API Server (Axum)
//! Exposes backup endpoints on standard port 8007.

use crate::datastore::{BackupManifest, Datastore};
use crate::sync::sync_from_remote;
use crate::verify::verify_snapshot;
use axum::{
    body::Body,
    extract::{Path as AxPath, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};

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
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

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
            post(upload_chunk_handler),
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
        .layer(cors)
        .with_state(state)
}

// Handlers

async fn status_handler(State(state): State<AppState>) -> impl IntoResponse {
    let stores = state.datastores.read().await;
    let names: Vec<String> = stores.keys().cloned().collect();
    Json(StatusResponse {
        ok: true,
        service: "podmesh-backup-server".to_string(),
        version: "2.1.0".to_string(),
        datastores: names,
        dedup_engine: "SHA-256 CAS 4MB (Pure Rust)".to_string(),
        op10_verifier_ready: true,
        remote_sync_enabled: true,
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
    let name = req.name.trim();
    if name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": "name requis" })),
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

    match ds.save_snapshot(&manifest) {
        Ok(_) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "ok": true, "snapshotId": manifest.id })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn get_chunk_handler(
    State(state): State<AppState>,
    AxPath((datastore, chunk_id)): AxPath<(String, String)>,
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
    State(state): State<AppState>,
    AxPath(datastore): AxPath<String>,
    Json(req): Json<SyncRequest>,
) -> Response {
    let stores = state.datastores.read().await;
    let ds = match stores.get(&datastore) {
        Some(d) => d.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "ok": false, "error": "Datastore introuvable" })),
            )
                .into_response()
        }
    };
    drop(stores);

    match sync_from_remote(&req.remote_url, &req.remote_datastore, &ds).await {
        Ok(report) => (StatusCode::OK, Json(report)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": e })),
        )
            .into_response(),
    }
}
