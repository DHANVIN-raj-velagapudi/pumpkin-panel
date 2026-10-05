// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
use crate::api::{load_all_servers, load_server, ServerRecord};
use crate::auth::{audit, server_access, CurrentUser, ServerAccess};
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::supervisor::RuntimeInfo;
use crate::AppState;
use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

#[derive(Debug, Serialize)]
pub struct ServerView {
    #[serde(flatten)]
    pub server: ServerRecord,
    pub runtime: RuntimeInfo,
    pub access: ServerAccess,
}

pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<Vec<ServerView>>> {
    let servers = load_all_servers(&state).await?;
    let mut views = Vec::new();

    for server in servers {
        // A user without a grant simply does not see the server.
        let Ok(access) = server_access(&state, &user, &server.id).await else {
            continue;
        };
        let runtime = state.sup.instance(&server.id).await.runtime().await;
        views.push(ServerView { server, runtime, access });
    }

    Ok(Json(views))
}

pub async fn detail(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<ServerView>> {
    let access = server_access(&state, &user, &id).await?;
    let server = load_server(&state, &id).await?;
    let runtime = state.sup.instance(&id).await.runtime().await;
    Ok(Json(ServerView { server, runtime, access }))
}

/// Links a registered server to a copy of it that is already running.
///
/// The panel normally only knows about processes it launched itself, but a
/// server is just as likely to be running already: started from a terminal,
/// left up across a panel reinstall, or registered after the fact. Without
/// this, such a server shows as stopped, and pressing Start tries to bind
/// ports that are taken.
///
/// An adopted process has no stdin pipe, so the console is read-only (it
/// follows the server's own log) until the server is restarted from the panel.
///
/// Returns whether a running process was found and linked.
pub async fn adopt_if_running(state: &AppState, server: &ServerRecord) -> bool {
    let spec = server.spec();
    let (binary, dir) = (spec.binary_path.clone(), spec.working_dir.clone());

    let Ok(Some((pid, started_at))) =
        tokio::task::spawn_blocking(move || crate::metrics::find_running(&binary, &dir)).await
    else {
        return false;
    };

    let instance = state.sup.instance(&server.id).await;
    if instance.adopt(&spec, pid, started_at).await.is_err() {
        // Already tracked, which is the normal case after a reattach.
        return false;
    }

    // Recorded like a panel launch, so the next panel restart reattaches too.
    let _ = sqlx::query(
        "INSERT OR REPLACE INTO running_servers
            (server_id, pid, started_at, binary_path)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(&server.id)
    .bind(i64::from(pid))
    .bind(started_at)
    .bind(&server.binary_path)
    .execute(&state.db)
    .await;

    tracing::info!(server = %server.name, pid, "linked to an already-running server");
    true
}

/// Runs [`adopt_if_running`] for every registered server. Called once at
/// startup, after the recorded servers are reattached and before autostart so
/// that autostart cannot launch a second copy of something already up.
pub async fn adopt_all_running(state: &AppState) {
    let Ok(servers) = load_all_servers(state).await else {
        return;
    };
    for server in servers {
        adopt_if_running(state, &server).await;
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateServer {
    pub name: String,
    pub binary_path: String,
    pub working_dir: String,
    #[serde(default)]
    pub args: Option<String>,
    #[serde(default)]
    pub stop_command: Option<String>,
    #[serde(default)]
    pub autostart: bool,
    #[serde(default)]
    pub memory_limit_mb: Option<i64>,
    #[serde(default)]
    pub disk_limit_mb: Option<i64>,
    #[serde(default)]
    pub cpu_cores: Option<i64>,
}

pub async fn create(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<CreateServer>,
) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;

    let name = body.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("name is required".into()));
    }

    let binary = PathBuf::from(body.binary_path.trim());
    if !binary.is_file() {
        return Err(AppError::BadRequest(format!(
            "no server binary at {}",
            binary.display()
        )));
    }

    // Default the working directory to wherever the binary lives.
    let working_dir = if body.working_dir.trim().is_empty() {
        binary
            .parent()
            .map(PathBuf::from)
            .ok_or_else(|| AppError::BadRequest("could not infer a working directory".into()))?
    } else {
        PathBuf::from(body.working_dir.trim())
    };
    if !working_dir.is_dir() {
        return Err(AppError::BadRequest(format!(
            "working directory does not exist: {}",
            working_dir.display()
        )));
    }

    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO servers
            (id, name, binary_path, working_dir, args, stop_command, autostart, created_at,
             memory_limit_mb, disk_limit_mb, cpu_cores)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )
    .bind(&id)
    .bind(name)
    .bind(binary.to_string_lossy().to_string())
    .bind(working_dir.to_string_lossy().to_string())
    .bind(body.args.unwrap_or_default())
    .bind(body.stop_command.unwrap_or_else(|| "stop".to_string()))
    .bind(i64::from(body.autostart))
    .bind(now())
    .bind(body.memory_limit_mb.unwrap_or(0).max(0))
    .bind(body.disk_limit_mb.unwrap_or(0).max(0))
    .bind(body.cpu_cores.unwrap_or(0).max(0))
    .execute(&state.db)
    .await?;

    audit(&state, Some(&user), Some(&id), "server.create", Some(name)).await;

    // If it is already running, link to it now rather than showing "stopped".
    let linked = match load_server(&state, &id).await {
        Ok(record) => adopt_if_running(&state, &record).await,
        Err(_) => false,
    };
    Ok(Json(json!({ "id": id, "linked_to_running": linked })))
}

#[derive(Debug, Deserialize)]
pub struct UpdateServer {
    pub name: Option<String>,
    pub binary_path: Option<String>,
    pub working_dir: Option<String>,
    pub args: Option<String>,
    pub stop_command: Option<String>,
    pub autostart: Option<bool>,
    pub memory_limit_mb: Option<i64>,
    pub disk_limit_mb: Option<i64>,
    pub cpu_cores: Option<i64>,
}

pub async fn update(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateServer>,
) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;
    load_server(&state, &id).await?;

    let mut sets: Vec<(&str, String)> = Vec::new();
    if let Some(v) = body.name {
        sets.push(("name", v));
    }
    if let Some(v) = body.binary_path {
        sets.push(("binary_path", v));
    }
    if let Some(v) = body.working_dir {
        sets.push(("working_dir", v));
    }
    if let Some(v) = body.args {
        sets.push(("args", v));
    }
    if let Some(v) = body.stop_command {
        sets.push(("stop_command", v));
    }
    if let Some(v) = body.autostart {
        sets.push(("autostart", i64::from(v).to_string()));
    }
    if let Some(v) = body.memory_limit_mb {
        sets.push(("memory_limit_mb", v.max(0).to_string()));
    }
    if let Some(v) = body.disk_limit_mb {
        sets.push(("disk_limit_mb", v.max(0).to_string()));
    }
    if let Some(v) = body.cpu_cores {
        sets.push(("cpu_cores", v.max(0).to_string()));
    }

    for (column, value) in sets {
        // Column names come from the fixed list above, never from user input.
        let sql = format!("UPDATE servers SET {column} = ?1 WHERE id = ?2");
        sqlx::query(&sql)
            .bind(value)
            .bind(&id)
            .execute(&state.db)
            .await?;
    }

    audit(&state, Some(&user), Some(&id), "server.update", None).await;
    Ok(Json(json!({ "ok": true })))
}

pub async fn remove(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;

    let instance = state.sup.instance(&id).await;
    let runtime = instance.runtime().await;
    if !matches!(
        runtime.status,
        crate::supervisor::Status::Stopped | crate::supervisor::Status::Crashed
    ) {
        return Err(AppError::Conflict(
            "stop the server before removing it".into(),
        ));
    }

    sqlx::query("DELETE FROM servers WHERE id = ?1")
        .bind(&id)
        .execute(&state.db)
        .await?;
    state.sup.forget(&id).await;

    audit(&state, Some(&user), Some(&id), "server.delete", None).await;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PowerAction {
    Start,
    Stop,
    Restart,
    Kill,
}

#[derive(Debug, Deserialize)]
pub struct PowerRequest {
    pub action: PowerAction,
}

pub async fn power(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<PowerRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let access = server_access(&state, &user, &id).await?;
    if !access.power {
        return Err(AppError::Forbidden);
    }

    let server = load_server(&state, &id).await?;
    let spec = server.spec();
    let instance = state.sup.instance(&id).await;

    let action = match body.action {
        PowerAction::Start => {
            instance.start(&spec).await?;
            // Remember the process so the panel can find it again after its own
            // restart. Recorded after a successful launch, never before.
            if let Some(pid) = instance.runtime().await.pid {
                let _ = sqlx::query(
                    "INSERT OR REPLACE INTO running_servers
                        (server_id, pid, started_at, binary_path)
                     VALUES (?1, ?2, ?3, ?4)",
                )
                .bind(&id)
                .bind(i64::from(pid))
                .bind(now())
                .bind(&server.binary_path)
                .execute(&state.db)
                .await;
            }
            "start"
        }
        PowerAction::Stop => {
            instance.stop(&spec).await?;
            "stop"
        }
        PowerAction::Restart => {
            instance.restart(&spec).await?;
            "restart"
        }
        PowerAction::Kill => {
            instance.kill().await;
            "kill"
        }
    };

    audit(
        &state,
        Some(&user),
        Some(&id),
        "server.power",
        Some(action),
    )
    .await;

    Ok(Json(json!({ "ok": true, "action": action })))
}

/// Starts every server flagged `autostart` when the panel boots.
pub async fn run_autostart(state: &AppState) {
    let servers = match load_all_servers(state).await {
        Ok(servers) => servers,
        Err(e) => {
            tracing::error!(error = %e, "autostart: could not read servers");
            return;
        }
    };

    for server in servers.into_iter().filter(|s| s.autostart) {
        let instance = state.sup.instance(&server.id).await;
        match instance.start(&server.spec()).await {
            Ok(()) => tracing::info!(server = %server.name, "autostarted"),
            Err(e) => tracing::warn!(server = %server.name, error = %e, "autostart failed"),
        }
    }
}
