//! The Activity page: reading the chained audit log and checking its integrity.

use crate::audit;
use crate::auth::CurrentUser;
use crate::error::AppResult;
use crate::AppState;
use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use sqlx::Row;

#[derive(Debug, Deserialize)]
pub struct ActivityQuery {
    /// One of the audit categories, or absent for everything.
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub server_id: Option<String>,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
    /// Sequence number to page backwards from.
    #[serde(default)]
    pub before: Option<i64>,
}

pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(query): Query<ActivityQuery>,
) -> AppResult<Json<serde_json::Value>> {
    // The activity log spans every server and account, so it is admin-only.
    user.require_admin()?;

    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    let category = query.category.filter(|c| c != "all");
    let search = query
        .search
        .filter(|s| !s.trim().is_empty())
        .map(|s| format!("%{}%", s.trim()));

    let rows = sqlx::query(
        "SELECT seq, at, actor, action, category, server_id, target, result,
                detail, meta, ip, request_id, hash, prev_hash
         FROM audit_events
         WHERE (?1 IS NULL OR category = ?1)
           AND (?2 IS NULL OR server_id = ?2)
           AND (?3 IS NULL OR actor LIKE ?3 OR action LIKE ?3 OR target LIKE ?3 OR detail LIKE ?3)
           AND (?4 IS NULL OR seq < ?4)
         ORDER BY seq DESC
         LIMIT ?5",
    )
    .bind(category)
    .bind(query.server_id)
    .bind(search)
    .bind(query.before)
    .bind(limit)
    .fetch_all(&state.db)
    .await?;

    let events: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            let meta: Option<String> = r.get("meta");
            json!({
                "seq": r.get::<i64, _>("seq"),
                "at": r.get::<i64, _>("at"),
                "actor": r.get::<String, _>("actor"),
                "action": r.get::<String, _>("action"),
                "category": r.get::<String, _>("category"),
                "server_id": r.get::<Option<String>, _>("server_id"),
                "target": r.get::<Option<String>, _>("target"),
                "result": r.get::<String, _>("result"),
                "detail": r.get::<Option<String>, _>("detail"),
                "meta": meta.and_then(|m| serde_json::from_str::<serde_json::Value>(&m).ok()),
                "ip": r.get::<Option<String>, _>("ip"),
                "request_id": r.get::<String, _>("request_id"),
                "hash": audit::short_hash(&r.get::<String, _>("hash")),
            })
        })
        .collect();

    let oldest = rows.last().map(|r| r.get::<i64, _>("seq"));

    Ok(Json(json!({
        "events": events,
        "oldest_seq": oldest,
        "has_more": rows.len() as i64 == limit,
    })))
}

/// Recomputes the whole hash chain and reports whether it is intact.
pub async fn verify(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<audit::IntegrityReport>> {
    user.require_admin()?;
    Ok(Json(audit::verify(&state).await?))
}

/// The current chain head, for recording somewhere off this machine.
pub async fn checkpoint(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<serde_json::Value>> {
    user.require_admin()?;
    let head = audit::checkpoint(&state).await?;
    Ok(Json(json!({
        "seq": head.as_ref().map(|(seq, _)| *seq),
        "hash": head.as_ref().map(|(_, hash)| hash.clone()),
        "note": "store this off the machine; if the local log is later rewritten, the head will no longer match",
    })))
}
