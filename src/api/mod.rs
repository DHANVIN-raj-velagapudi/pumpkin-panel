// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
pub mod activity;
pub mod backups;
pub mod config;
pub mod players;
pub mod pumpkin;
pub mod console;
pub mod files;
pub mod mfa;
pub mod servers;
pub mod session;
pub mod stats;
pub mod users;

use crate::error::{AppError, AppResult};
use crate::supervisor::ServerSpec;
use crate::AppState;
use axum::routing::{delete, get, post, put};
use axum::Router;
use serde::Serialize;
use sqlx::Row;
use std::path::PathBuf;

/// One row of the `servers` table.
#[derive(Debug, Clone, Serialize)]
pub struct ServerRecord {
    pub id: String,
    pub name: String,
    pub binary_path: String,
    pub working_dir: String,
    pub args: String,
    pub stop_command: String,
    pub autostart: bool,
    pub created_at: i64,
    /// 0 means no limit. Pumpkin has no heap flag, so this is a monitored
    /// ceiling rather than an allocation.
    pub memory_limit_mb: i64,
    pub disk_limit_mb: i64,
    /// 0 means every core on the machine.
    pub cpu_cores: i64,
    /// off | hourly | daily | weekly
    pub backup_schedule: String,
    pub backup_keep: i64,
}

impl ServerRecord {
    pub fn spec(&self) -> ServerSpec {
        ServerSpec {
            id: self.id.clone(),
            binary_path: PathBuf::from(&self.binary_path),
            working_dir: PathBuf::from(&self.working_dir),
            args: self
                .args
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
            stop_command: self.stop_command.clone(),
            cpu_cores: self.cpu_cores.max(0) as usize,
        }
    }

    fn from_row(row: &sqlx::sqlite::SqliteRow) -> Self {
        Self {
            id: row.get("id"),
            name: row.get("name"),
            binary_path: row.get("binary_path"),
            working_dir: row.get("working_dir"),
            args: row.get("args"),
            stop_command: row.get("stop_command"),
            autostart: row.get::<i64, _>("autostart") != 0,
            created_at: row.get("created_at"),
            memory_limit_mb: row.try_get("memory_limit_mb").unwrap_or(0),
            disk_limit_mb: row.try_get("disk_limit_mb").unwrap_or(0),
            cpu_cores: row.try_get("cpu_cores").unwrap_or(0),
            backup_schedule: row
                .try_get("backup_schedule")
                .unwrap_or_else(|_| "off".to_string()),
            backup_keep: row.try_get("backup_keep").unwrap_or(7),
        }
    }
}

pub async fn load_server(state: &AppState, id: &str) -> AppResult<ServerRecord> {
    let row = sqlx::query("SELECT * FROM servers WHERE id = ?1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("server".into()))?;
    Ok(ServerRecord::from_row(&row))
}

pub async fn load_all_servers(state: &AppState) -> AppResult<Vec<ServerRecord>> {
    let rows = sqlx::query("SELECT * FROM servers ORDER BY name COLLATE NOCASE")
        .fetch_all(&state.db)
        .await?;
    Ok(rows.iter().map(ServerRecord::from_row).collect())
}

pub fn router() -> Router<AppState> {
    Router::new()
        // session
        .route("/auth/login", post(session::login))
        .route("/auth/logout", post(session::logout))
        .route("/auth/logout-all", post(session::logout_everywhere))
        // two-factor authentication
        .route("/auth/2fa", get(mfa::status))
        .route("/auth/2fa/setup", post(mfa::setup))
        .route("/auth/2fa/enable", post(mfa::enable))
        .route("/auth/2fa/disable", post(mfa::disable))
        .route("/auth/me", get(session::me))
        // users (admin only)
        .route("/users", get(users::list).post(users::create))
        .route("/users/{id}", delete(users::remove).patch(users::update))
        .route("/users/{id}/servers", get(users::list_grants).put(users::set_grant))
        // servers
        .route("/servers", get(servers::list).post(servers::create))
        .route(
            "/servers/{id}",
            get(servers::detail).patch(servers::update).delete(servers::remove),
        )
        .route("/servers/{id}/power", post(servers::power))
        .route("/servers/{id}/stats", get(stats::stats))
        .route("/servers/{id}/stats/history", get(stats::history))
        // players
        .route("/servers/{id}/players", get(players::list))
        .route("/servers/{id}/players/action", post(players::act))
        // pumpkin-specific
        .route("/servers/{id}/pumpkin", get(pumpkin::overview))
        .route("/marketplace", get(pumpkin::marketplace))
        // console
        .route("/servers/{id}/console", get(console::history).post(console::command))
        .route("/servers/{id}/console/ws", get(console::websocket))
        // files
        .route("/servers/{id}/files", get(files::list))
        .route(
            "/servers/{id}/files/content",
            get(files::read).put(files::write),
        )
        .route("/servers/{id}/files/mkdir", post(files::mkdir))
        .route("/servers/{id}/files/rename", post(files::rename))
        .route("/servers/{id}/files/delete", post(files::remove))
        .route(
            "/servers/{id}/files/upload",
            // The default body cap is 2 MB, far too small for world uploads.
            post(files::upload).layer(axum::extract::DefaultBodyLimit::max(1024 * 1024 * 1024)),
        )
        .route("/servers/{id}/files/download", get(files::download))
        .route("/servers/{id}/files/archive", post(files::archive))
        .route("/servers/{id}/files/extract", post(files::extract))
        .route("/servers/{id}/files/copy", post(files::copy))
        // pumpkin.toml aware config editing
        .route("/servers/{id}/config", get(config::read).put(config::write))
        .route("/servers/{id}/config/files", get(config::list_configs))
        // backups
        .route("/servers/{id}/backups", get(backups::list).post(backups::create))
        .route("/servers/{id}/backups/schedule", put(backups::set_schedule))
        .route("/servers/{id}/backups/{backup}", delete(backups::remove))
        .route("/servers/{id}/backups/{backup}/restore", post(backups::restore))
        .route("/servers/{id}/backups/{backup}/download", get(backups::download))
        // activity log
        .route("/activity", get(activity::list))
        .route("/activity/verify", get(activity::verify))
        .route("/activity/checkpoint", get(activity::checkpoint))
        .route("/audit", get(users::audit_log))
        .route("/health", get(health))
}

async fn health() -> &'static str {
    "ok"
}
