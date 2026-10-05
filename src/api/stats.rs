// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
//! Resource usage for a running server.
//!
//! Pumpkin is a native binary, not a JVM, so there is no `-Xmx` to set: memory
//! is whatever the process asks the OS for. The panel therefore *measures*
//! usage and compares it against the limits configured for the server, rather
//! than pretending it can pre-allocate a heap.

use crate::api::load_server;
use crate::auth::{server_access, CurrentUser};
use crate::error::AppResult;
use crate::AppState;
use axum::extract::{Path, State};
use axum::Json;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// Walking a big world folder is slow, so the figure is reused for a minute.
const DISK_CACHE_SECS: i64 = 60;

type DiskCache = Mutex<HashMap<String, (i64, u64)>>;

fn disk_cache() -> &'static DiskCache {
    static CACHE: OnceLock<DiskCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug, Serialize)]
pub struct Stats {
    pub running: bool,
    pub memory_bytes: u64,
    pub cpu_percent: f32,
    pub disk_bytes: u64,
    pub memory_limit_mb: i64,
    pub disk_limit_mb: i64,
    /// 0 means every core.
    pub cpu_cores: i64,
    /// Total physical memory on the host, for context in the UI.
    pub host_memory_bytes: u64,
    /// Logical processors available on this machine.
    pub host_cores: usize,
}

fn directory_size(root: PathBuf) -> u64 {
    walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.metadata().ok())
        .filter(std::fs::Metadata::is_file)
        .map(|meta| meta.len())
        .sum()
}

async fn cached_directory_size(id: &str, root: PathBuf) -> u64 {
    let now = crate::db::now();

    if let Ok(cache) = disk_cache().lock() {
        if let Some((measured_at, bytes)) = cache.get(id) {
            if now - measured_at < DISK_CACHE_SECS {
                return *bytes;
            }
        }
    }

    let bytes = tokio::task::spawn_blocking(move || directory_size(root))
        .await
        .unwrap_or(0);

    if let Ok(mut cache) = disk_cache().lock() {
        cache.insert(id.to_string(), (now, bytes));
    }
    bytes
}

pub async fn stats(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<Stats>> {
    // Anyone who can see the server may see how much it is using.
    server_access(&state, &user, &id).await?;
    let server = load_server(&state, &id).await?;

    let runtime = state.sup.instance(&id).await.runtime().await;
    let (memory_bytes, cpu_percent) = match runtime.pid {
        Some(pid) => tokio::task::spawn_blocking(move || crate::metrics::sample_process(pid))
            .await
            .unwrap_or((0, 0.0)),
        None => (0, 0.0),
    };

    let disk_bytes = cached_directory_size(&id, PathBuf::from(&server.working_dir)).await;
    let host_memory_bytes = tokio::task::spawn_blocking(crate::metrics::host_memory)
        .await
        .unwrap_or(0);

    Ok(Json(Stats {
        running: runtime.pid.is_some(),
        memory_bytes,
        cpu_percent,
        disk_bytes,
        memory_limit_mb: server.memory_limit_mb,
        disk_limit_mb: server.disk_limit_mb,
        cpu_cores: server.cpu_cores,
        host_memory_bytes,
        host_cores: crate::affinity::available_cores(),
    }))
}

/// Recorded CPU and memory samples, for the Overview charts.
pub async fn history(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    server_access(&state, &user, &id).await?;
    let samples = state.sup.instance(&id).await.samples().await;
    Ok(Json(serde_json::json!({ "samples": samples })))
}
