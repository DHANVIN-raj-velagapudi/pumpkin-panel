// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
//! Backup endpoints: snapshot, list, restore, prune.

use crate::api::load_server;
use crate::audit::{Category, Event, Outcome};
use crate::auth::{server_access, CurrentUser};
use crate::backup;
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::AppState;
use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use sqlx::Row;
use std::path::PathBuf;

/// Backups can rewrite the world, so file access is the minimum bar.
async fn require_files_access(state: &AppState, user: &CurrentUser, id: &str) -> AppResult<()> {
    if !server_access(state, user, id).await?.files {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

fn archive_path(state: &AppState, server_id: &str, backup_id: &str) -> PathBuf {
    backup::backup_dir(&state.data_dir, server_id).join(format!("{backup_id}.zip"))
}

pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    require_files_access(&state, &user, &id).await?;
    let server = load_server(&state, &id).await?;

    let rows = sqlx::query(
        "SELECT id, created_at, size_bytes, file_count, kind, note
         FROM backups WHERE server_id = ?1 ORDER BY created_at DESC",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await?;

    let backups: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            json!({
                "id": r.get::<String, _>("id"),
                "created_at": r.get::<i64, _>("created_at"),
                "size_bytes": r.get::<i64, _>("size_bytes"),
                "file_count": r.get::<i64, _>("file_count"),
                "kind": r.get::<String, _>("kind"),
                "note": r.get::<Option<String>, _>("note"),
            })
        })
        .collect();

    let total: i64 = rows.iter().map(|r| r.get::<i64, _>("size_bytes")).sum();

    Ok(Json(json!({
        "backups": backups,
        "total_bytes": total,
        "schedule": server.backup_schedule,
        "keep": server.backup_keep,
    })))
}

#[derive(Debug, Deserialize)]
pub struct CreateRequest {
    #[serde(default)]
    pub note: Option<String>,
}

pub async fn create(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<CreateRequest>,
) -> AppResult<Json<serde_json::Value>> {
    require_files_access(&state, &user, &id).await?;
    let record = snapshot(&state, &id, "manual", body.note.as_deref()).await?;

    crate::audit::record(
        &state,
        Event::new("BACKUP_CREATED", Category::Backups, Outcome::Success)
            .actor(&user)
            .server(&id)
            .target(&record.id)
            .detail(format!(
                "{} files, {:.1} MB",
                record.file_count,
                record.size_bytes as f64 / 1024.0 / 1024.0
            )),
    )
    .await;

    Ok(Json(json!({ "ok": true, "backup": record })))
}

/// Takes a snapshot and records it. Shared by manual, scheduled and
/// pre-restore paths.
pub async fn snapshot(
    state: &AppState,
    server_id: &str,
    kind: &str,
    note: Option<&str>,
) -> AppResult<backup::BackupRecord> {
    let server = load_server(state, server_id).await?;
    let backup_id = uuid::Uuid::new_v4().simple().to_string();
    let destination = archive_path(state, server_id, &backup_id);

    let working_dir = PathBuf::from(&server.working_dir);
    let binary = PathBuf::from(&server.binary_path);
    let target = destination.clone();

    // Zipping a world is slow and entirely blocking work.
    let result = tokio::task::spawn_blocking(move || {
        backup::create_snapshot(&working_dir, &binary, &target)
    })
    .await
    .map_err(|e| AppError::Other(anyhow::anyhow!("backup task failed: {e}")))?
    .map_err(AppError::BadRequest)?;

    let created_at = now();
    sqlx::query(
        "INSERT INTO backups (id, server_id, created_at, size_bytes, file_count, kind, note)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(&backup_id)
    .bind(server_id)
    .bind(created_at)
    .bind(result.size_bytes as i64)
    .bind(result.file_count)
    .bind(kind)
    .bind(note)
    .execute(&state.db)
    .await?;

    Ok(backup::BackupRecord {
        id: backup_id,
        server_id: server_id.to_string(),
        created_at,
        size_bytes: result.size_bytes as i64,
        file_count: result.file_count,
        kind: kind.to_string(),
        note: note.map(str::to_owned),
    })
}

pub async fn restore(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, backup_id)): Path<(String, String)>,
) -> AppResult<Json<serde_json::Value>> {
    // Restoring overwrites the world for everyone, so it is administrator-only.
    user.require_admin()?;
    require_files_access(&state, &user, &id).await?;

    // Writing over a world while the server holds it open would corrupt both.
    let runtime = state.sup.instance(&id).await.runtime().await;
    if !matches!(
        runtime.status,
        crate::supervisor::Status::Stopped | crate::supervisor::Status::Crashed
    ) {
        return Err(AppError::Conflict(
            "stop the server before restoring a backup".into(),
        ));
    }

    let exists: Option<String> = sqlx::query("SELECT id FROM backups WHERE id = ?1 AND server_id = ?2")
        .bind(&backup_id)
        .bind(&id)
        .fetch_optional(&state.db)
        .await?
        .map(|r| r.get("id"));

    if exists.is_none() {
        return Err(AppError::NotFound("backup".into()));
    }

    // A snapshot of the current state first, so restoring the wrong backup is
    // recoverable rather than final.
    let safety = snapshot(&state, &id, "pre-restore", Some("taken automatically before a restore"))
        .await?;

    let server = load_server(&state, &id).await?;
    let archive = archive_path(&state, &id, &backup_id);
    let working_dir = PathBuf::from(&server.working_dir);

    let restored = tokio::task::spawn_blocking(move || {
        backup::restore_snapshot(&archive, &working_dir)
    })
    .await
    .map_err(|e| AppError::Other(anyhow::anyhow!("restore task failed: {e}")))?
    .map_err(AppError::BadRequest)?;

    crate::audit::record(
        &state,
        Event::new("BACKUP_RESTORED", Category::Backups, Outcome::Success)
            .actor(&user)
            .server(&id)
            .target(&backup_id)
            .detail(format!("{restored} files restored"))
            .meta(json!({ "safety_backup": safety.id })),
    )
    .await;

    Ok(Json(json!({
        "ok": true,
        "restored": restored,
        "safety_backup": safety.id,
        "note": "the previous state was saved as a pre-restore backup",
    })))
}

pub async fn remove(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, backup_id)): Path<(String, String)>,
) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;
    require_files_access(&state, &user, &id).await?;

    let result = sqlx::query("DELETE FROM backups WHERE id = ?1 AND server_id = ?2")
        .bind(&backup_id)
        .bind(&id)
        .execute(&state.db)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("backup".into()));
    }

    let _ = tokio::fs::remove_file(archive_path(&state, &id, &backup_id)).await;

    crate::audit::record(
        &state,
        Event::new("BACKUP_DELETED", Category::Backups, Outcome::Success)
            .actor(&user)
            .server(&id)
            .target(&backup_id),
    )
    .await;

    Ok(Json(json!({ "ok": true })))
}

pub async fn download(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, backup_id)): Path<(String, String)>,
) -> AppResult<axum::response::Response> {
    use axum::http::header;
    use axum::response::IntoResponse;

    require_files_access(&state, &user, &id).await?;

    let path = archive_path(&state, &id, &backup_id);
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| AppError::NotFound("backup".into()))?;

    crate::audit::record(
        &state,
        Event::new("BACKUP_DOWNLOADED", Category::Backups, Outcome::Success)
            .actor(&user)
            .server(&id)
            .target(&backup_id),
    )
    .await;

    Ok((
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"backup-{backup_id}.zip\""),
            ),
        ],
        bytes,
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
pub struct ScheduleRequest {
    /// off | hourly | daily | weekly
    pub schedule: String,
    /// How many scheduled snapshots to keep.
    pub keep: i64,
}

pub async fn set_schedule(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<ScheduleRequest>,
) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;
    let schedule = backup::parse_schedule(&body.schedule)?;
    let keep = body.keep.clamp(1, 100);

    sqlx::query("UPDATE servers SET backup_schedule = ?1, backup_keep = ?2 WHERE id = ?3")
        .bind(&schedule)
        .bind(keep)
        .bind(&id)
        .execute(&state.db)
        .await?;

    crate::audit::record(
        &state,
        Event::new("BACKUP_SCHEDULE_CHANGED", Category::Backups, Outcome::Success)
            .actor(&user)
            .server(&id)
            .detail(format!("{schedule}, keeping {keep}")),
    )
    .await;

    Ok(Json(json!({ "ok": true, "schedule": schedule, "keep": keep })))
}

/// Background sweep: takes any snapshot that has come due and prunes old ones.
pub async fn run_scheduler(state: AppState) {
    // Checked every minute; the due calculation itself is what enforces the
    // hourly/daily/weekly spacing.
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));

    loop {
        ticker.tick().await;

        let servers = match crate::api::load_all_servers(&state).await {
            Ok(servers) => servers,
            Err(e) => {
                tracing::warn!(error = %e, "backup scheduler could not read servers");
                continue;
            }
        };

        for server in servers {
            if server.backup_schedule == "off" {
                continue;
            }

            let last: Option<i64> = sqlx::query(
                "SELECT MAX(created_at) AS last FROM backups
                 WHERE server_id = ?1 AND kind = 'scheduled'",
            )
            .bind(&server.id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
            .and_then(|r| r.try_get("last").ok());

            if !backup::is_due(&server.backup_schedule, last) {
                continue;
            }

            match snapshot(&state, &server.id, "scheduled", None).await {
                Ok(record) => {
                    tracing::info!(
                        server = %server.name,
                        size = record.size_bytes,
                        "scheduled backup taken"
                    );
                    crate::audit::record(
                        &state,
                        Event::new("BACKUP_CREATED", Category::Backups, Outcome::Success)
                            .actor_name("scheduler")
                            .server(&server.id)
                            .target(&record.id)
                            .detail(format!("{} scheduled snapshot", server.backup_schedule)),
                    )
                    .await;
                }
                Err(e) => {
                    tracing::warn!(server = %server.name, error = %e, "scheduled backup failed");
                    crate::audit::record(
                        &state,
                        Event::new("BACKUP_FAILED", Category::Backups, Outcome::Failure)
                            .actor_name("scheduler")
                            .server(&server.id)
                            .detail(e.to_string()),
                    )
                    .await;
                    continue;
                }
            }

            match backup::apply_retention(
                &state.db,
                &state.data_dir,
                &server.id,
                server.backup_keep,
            )
            .await
            {
                Ok(0) => {}
                Ok(removed) => tracing::info!(
                    server = %server.name,
                    removed,
                    "pruned old scheduled backups"
                ),
                Err(e) => tracing::warn!(error = %e, "backup retention failed"),
            }
        }
    }
}
