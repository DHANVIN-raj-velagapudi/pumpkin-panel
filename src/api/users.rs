// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
use crate::auth::{audit, hash_password, CurrentUser, Role};
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;

#[derive(Debug, Serialize)]
pub struct UserSummary {
    pub id: String,
    pub username: String,
    pub email: Option<String>,
    pub role: String,
    pub is_active: bool,
    pub created_at: i64,
}

pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<Vec<UserSummary>>> {
    user.require_admin()?;

    let rows = sqlx::query(
        "SELECT id, username, email, role, is_active, created_at
         FROM users ORDER BY username COLLATE NOCASE",
    )
    .fetch_all(&state.db)
    .await?;

    Ok(Json(
        rows.iter()
            .map(|r| UserSummary {
                id: r.get("id"),
                username: r.get("username"),
                email: r.get("email"),
                role: r.get("role"),
                is_active: r.get::<i64, _>("is_active") != 0,
                created_at: r.get("created_at"),
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
pub struct CreateUser {
    pub username: String,
    pub password: String,
    pub email: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
}

pub async fn create(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<CreateUser>,
) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;

    let username = body.username.trim();
    if username.is_empty() {
        return Err(AppError::BadRequest("username is required".into()));
    }
    if body.password.len() < 8 {
        return Err(AppError::BadRequest(
            "password must be at least 8 characters".into(),
        ));
    }

    let role = Role::parse(body.role.as_deref().unwrap_or("user"));
    let id = uuid::Uuid::new_v4().to_string();

    let result = sqlx::query(
        "INSERT INTO users (id, username, email, password_hash, role, is_active, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
    )
    .bind(&id)
    .bind(username)
    .bind(body.email.as_deref())
    .bind(hash_password(&body.password)?)
    .bind(role.as_str())
    .bind(now())
    .execute(&state.db)
    .await;

    if let Err(sqlx::Error::Database(e)) = &result {
        if e.message().contains("UNIQUE") {
            return Err(AppError::Conflict("that username is already taken".into()));
        }
    }
    result?;

    audit(&state, Some(&user), None, "user.create", Some(username)).await;
    Ok(Json(json!({ "id": id })))
}

#[derive(Debug, Deserialize)]
pub struct UpdateUser {
    pub password: Option<String>,
    pub email: Option<String>,
    pub role: Option<String>,
    pub is_active: Option<bool>,
}

pub async fn update(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateUser>,
) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;

    if let Some(password) = &body.password {
        if password.len() < 8 {
            return Err(AppError::BadRequest(
                "password must be at least 8 characters".into(),
            ));
        }
        sqlx::query("UPDATE users SET password_hash = ?1 WHERE id = ?2")
            .bind(hash_password(password)?)
            .bind(&id)
            .execute(&state.db)
            .await?;
        // Force a fresh login everywhere after a password change.
        sqlx::query("DELETE FROM sessions WHERE user_id = ?1")
            .bind(&id)
            .execute(&state.db)
            .await?;
    }
    if let Some(email) = &body.email {
        sqlx::query("UPDATE users SET email = ?1 WHERE id = ?2")
            .bind(email)
            .bind(&id)
            .execute(&state.db)
            .await?;
    }
    if let Some(role) = &body.role {
        sqlx::query("UPDATE users SET role = ?1 WHERE id = ?2")
            .bind(Role::parse(role).as_str())
            .bind(&id)
            .execute(&state.db)
            .await?;
    }
    if let Some(active) = body.is_active {
        if !active && id == user.id {
            return Err(AppError::BadRequest(
                "you cannot deactivate your own account".into(),
            ));
        }
        sqlx::query("UPDATE users SET is_active = ?1 WHERE id = ?2")
            .bind(i64::from(active))
            .bind(&id)
            .execute(&state.db)
            .await?;
    }

    audit(&state, Some(&user), None, "user.update", Some(&id)).await;
    Ok(Json(json!({ "ok": true })))
}

pub async fn remove(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;
    if id == user.id {
        return Err(AppError::BadRequest(
            "you cannot delete your own account".into(),
        ));
    }

    sqlx::query("DELETE FROM users WHERE id = ?1")
        .bind(&id)
        .execute(&state.db)
        .await?;

    audit(&state, Some(&user), None, "user.delete", Some(&id)).await;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Serialize)]
pub struct Grant {
    pub server_id: String,
    pub can_console: bool,
    pub can_power: bool,
    pub can_files: bool,
    pub can_config: bool,
}

pub async fn list_grants(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<Vec<Grant>>> {
    user.require_admin()?;

    let rows = sqlx::query("SELECT * FROM server_permissions WHERE user_id = ?1")
        .bind(&id)
        .fetch_all(&state.db)
        .await?;

    Ok(Json(
        rows.iter()
            .map(|r| Grant {
                server_id: r.get("server_id"),
                can_console: r.get::<i64, _>("can_console") != 0,
                can_power: r.get::<i64, _>("can_power") != 0,
                can_files: r.get::<i64, _>("can_files") != 0,
                can_config: r.get::<i64, _>("can_config") != 0,
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
pub struct SetGrant {
    pub server_id: String,
    #[serde(default)]
    pub can_console: bool,
    #[serde(default)]
    pub can_power: bool,
    #[serde(default)]
    pub can_files: bool,
    #[serde(default)]
    pub can_config: bool,
    /// When true the grant is removed entirely.
    #[serde(default)]
    pub revoke: bool,
}

pub async fn set_grant(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<SetGrant>,
) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;

    if body.revoke {
        sqlx::query("DELETE FROM server_permissions WHERE user_id = ?1 AND server_id = ?2")
            .bind(&id)
            .bind(&body.server_id)
            .execute(&state.db)
            .await?;
    } else {
        sqlx::query(
            "INSERT INTO server_permissions
                (user_id, server_id, can_console, can_power, can_files, can_config)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(user_id, server_id) DO UPDATE SET
                can_console = excluded.can_console,
                can_power   = excluded.can_power,
                can_files   = excluded.can_files,
                can_config  = excluded.can_config",
        )
        .bind(&id)
        .bind(&body.server_id)
        .bind(i64::from(body.can_console))
        .bind(i64::from(body.can_power))
        .bind(i64::from(body.can_files))
        .bind(i64::from(body.can_config))
        .execute(&state.db)
        .await?;
    }

    audit(
        &state,
        Some(&user),
        Some(&body.server_id),
        "user.grant",
        Some(&id),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    #[serde(default)]
    pub limit: Option<i64>,
}

pub async fn audit_log(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(query): Query<AuditQuery>,
) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;
    let limit = query.limit.unwrap_or(200).clamp(1, 1_000);

    let rows = sqlx::query(
        "SELECT id, username, server_id, action, detail, at
         FROM audit_log ORDER BY at DESC, id DESC LIMIT ?1",
    )
    .bind(limit)
    .fetch_all(&state.db)
    .await?;

    let entries: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            json!({
                "id": r.get::<i64, _>("id"),
                "username": r.get::<Option<String>, _>("username"),
                "server_id": r.get::<Option<String>, _>("server_id"),
                "action": r.get::<String, _>("action"),
                "detail": r.get::<Option<String>, _>("detail"),
                "at": r.get::<i64, _>("at"),
            })
        })
        .collect();

    Ok(Json(json!({ "entries": entries })))
}
