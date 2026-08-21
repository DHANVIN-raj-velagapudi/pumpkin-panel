//! Tamper-evident activity log.
//!
//! Every event carries the hash of the one before it, so the log forms a chain.
//! Editing a record in the middle breaks every hash after it, and the panel can
//! say exactly where the break is.
//!
//! This is deliberately *tamper-evident*, not tamper-proof: anyone with
//! administrator access to the host can rewrite the database and recompute the
//! whole chain. What it stops is quiet edits — someone deleting the line that
//! records what they did. Pairing it with an off-machine checkpoint (see
//! [`checkpoint`]) is what closes that gap, because the attacker would also
//! have to alter a copy they do not control.
//!
//! The log lives in `panel.db`, inside the panel's own data directory. That is
//! outside every managed server folder, and the file manager is sandboxed to
//! those folders, so the log cannot be reached through the panel's own file
//! browser.

use crate::db::now;
use crate::error::AppResult;
use crate::AppState;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::Row;

/// Broad grouping, used by the Activity page filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Security,
    Servers,
    Files,
    Config,
    Players,
    Users,
    Backups,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Security => "security",
            Self::Servers => "servers",
            Self::Files => "files",
            Self::Config => "config",
            Self::Players => "players",
            Self::Users => "users",
            Self::Backups => "backups",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Success,
    Failure,
    Denied,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "SUCCESS",
            Self::Failure => "FAILURE",
            Self::Denied => "DENIED",
        }
    }
}

/// One activity record, before it is chained and written.
#[derive(Debug, Clone)]
pub struct Event {
    pub actor_id: Option<String>,
    pub actor: String,
    pub action: String,
    pub category: Category,
    pub server_id: Option<String>,
    pub target: Option<String>,
    pub outcome: Outcome,
    pub detail: Option<String>,
    /// Structured extras: sizes, hashes, before/after. Never secrets.
    pub meta: Option<serde_json::Value>,
    pub ip: Option<String>,
}

impl Event {
    pub fn new(action: &str, category: Category, outcome: Outcome) -> Self {
        Self {
            actor_id: None,
            actor: "system".to_string(),
            action: action.to_string(),
            category,
            server_id: None,
            target: None,
            outcome,
            detail: None,
            meta: None,
            ip: None,
        }
    }

    pub fn actor(mut self, user: &crate::auth::CurrentUser) -> Self {
        self.actor_id = Some(user.id.clone());
        self.actor = user.username.clone();
        self
    }

    /// For events with no signed-in user yet, such as a failed sign-in.
    pub fn actor_name(mut self, name: &str) -> Self {
        self.actor = name.to_string();
        self
    }

    pub fn server(mut self, id: &str) -> Self {
        self.server_id = Some(id.to_string());
        self
    }

    pub fn target(mut self, target: &str) -> Self {
        self.target = Some(target.to_string());
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn meta(mut self, meta: serde_json::Value) -> Self {
        self.meta = Some(meta);
        self
    }

    pub fn ip(mut self, ip: Option<String>) -> Self {
        self.ip = ip;
        self
    }
}

/// SHA-256 of a file's contents, used to record what changed without storing
/// the contents themselves.
pub fn hash_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Short form for display.
pub fn short_hash(full: &str) -> String {
    full.chars().take(12).collect()
}

/// Hashes an event for writing. Delegates to the same primitive the verifier
/// uses, so the two can never drift apart.
fn compute_hash(
    seq: i64,
    at: i64,
    event: &Event,
    request_id: &str,
    meta_json: &str,
    prev_hash: &str,
) -> String {
    compute_hash_raw(
        seq,
        at,
        &event.actor,
        &event.action,
        event.category.as_str(),
        event.server_id.as_deref().unwrap_or(""),
        event.target.as_deref().unwrap_or(""),
        event.outcome.as_str(),
        event.detail.as_deref().unwrap_or(""),
        meta_json,
        event.ip.as_deref().unwrap_or(""),
        request_id,
        prev_hash,
    )
}

/// Appends an event, linking it to the previous one.
///
/// Failures are logged and swallowed: an audit write must never take down the
/// action it is describing.
pub async fn record(state: &AppState, event: Event) {
    if let Err(e) = try_record(state, event).await {
        tracing::error!(error = %e, "failed to append an audit event");
    }
}

async fn try_record(state: &AppState, event: Event) -> AppResult<()> {
    // Serialised so two concurrent writers cannot both build on the same
    // predecessor and fork the chain.
    let _guard = state.audit_lock.lock().await;

    let previous: Option<String> = sqlx::query("SELECT hash FROM audit_events ORDER BY seq DESC LIMIT 1")
        .fetch_optional(&state.db)
        .await?
        .map(|row| row.get("hash"));

    // The first record chains from a fixed origin value.
    let prev_hash = previous.unwrap_or_else(|| "genesis".to_string());

    let next_seq: i64 = sqlx::query("SELECT COALESCE(MAX(seq), 0) + 1 AS next FROM audit_events")
        .fetch_one(&state.db)
        .await?
        .get("next");

    let at = now();
    let request_id = uuid::Uuid::new_v4().simple().to_string();
    let meta_json = event
        .meta
        .as_ref()
        .map(|m| m.to_string())
        .unwrap_or_default();

    let hash = compute_hash(next_seq, at, &event, &request_id, &meta_json, &prev_hash);

    sqlx::query(
        "INSERT INTO audit_events
            (seq, at, actor_id, actor, action, category, server_id, target,
             result, detail, meta, ip, request_id, prev_hash, hash)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
    )
    .bind(next_seq)
    .bind(at)
    .bind(&event.actor_id)
    .bind(&event.actor)
    .bind(&event.action)
    .bind(event.category.as_str())
    .bind(&event.server_id)
    .bind(&event.target)
    .bind(event.outcome.as_str())
    .bind(&event.detail)
    .bind(if meta_json.is_empty() { None } else { Some(meta_json.as_str()) })
    .bind(&event.ip)
    .bind(&request_id)
    .bind(&prev_hash)
    .bind(&hash)
    .execute(&state.db)
    .await?;

    Ok(())
}

#[derive(Debug, Serialize)]
pub struct IntegrityReport {
    pub checked: i64,
    pub intact: bool,
    /// Sequence number of the first record that does not match.
    pub broken_at: Option<i64>,
    pub message: String,
    /// Hash of the newest record, suitable for an off-machine checkpoint.
    pub head: Option<String>,
}

/// Recomputes the whole chain and reports the first record that fails.
pub async fn verify(state: &AppState) -> AppResult<IntegrityReport> {
    let rows = sqlx::query(
        "SELECT seq, at, actor, action, category, server_id, target, result,
                detail, meta, ip, request_id, prev_hash, hash
         FROM audit_events ORDER BY seq ASC",
    )
    .fetch_all(&state.db)
    .await?;

    let mut expected_prev = "genesis".to_string();
    let mut checked = 0i64;
    let mut head = None;

    for row in &rows {
        let seq: i64 = row.get("seq");
        let stored_prev: String = row.get("prev_hash");
        let stored_hash: String = row.get("hash");

        if stored_prev != expected_prev {
            return Ok(IntegrityReport {
                checked,
                intact: false,
                broken_at: Some(seq),
                message: format!("record {seq} does not follow the one before it; a record was altered or removed"),
                head,
            });
        }

        // Everything is rehashed from the stored text, so the check does not
        // depend on any enum round-tripping correctly.
        let actor: String = row.get("actor");
        let action: String = row.get("action");
        let server_id: Option<String> = row.get("server_id");
        let target: Option<String> = row.get("target");
        let detail: Option<String> = row.get("detail");
        let ip: Option<String> = row.get("ip");
        let category: String = row.get("category");
        let result: String = row.get("result");
        let meta: Option<String> = row.get("meta");
        let request_id: String = row.get("request_id");
        let at: i64 = row.get("at");

        let recomputed = compute_hash_raw(
            seq,
            at,
            &actor,
            &action,
            &category,
            server_id.as_deref().unwrap_or(""),
            target.as_deref().unwrap_or(""),
            &result,
            detail.as_deref().unwrap_or(""),
            meta.as_deref().unwrap_or(""),
            ip.as_deref().unwrap_or(""),
            &request_id,
            &stored_prev,
        );

        if recomputed != stored_hash {
            return Ok(IntegrityReport {
                checked,
                intact: false,
                broken_at: Some(seq),
                message: format!("record {seq} has been modified since it was written"),
                head,
            });
        }

        expected_prev = stored_hash.clone();
        head = Some(stored_hash);
        checked += 1;
    }

    Ok(IntegrityReport {
        checked,
        intact: true,
        broken_at: None,
        message: if checked == 0 {
            "no activity recorded yet".to_string()
        } else {
            format!("all {checked} records verified, chain intact")
        },
        head,
    })
}

/// The hashing primitive, taking already-stringified fields.
#[allow(clippy::too_many_arguments)]
fn compute_hash_raw(
    seq: i64,
    at: i64,
    actor: &str,
    action: &str,
    category: &str,
    server_id: &str,
    target: &str,
    result: &str,
    detail: &str,
    meta: &str,
    ip: &str,
    request_id: &str,
    prev_hash: &str,
) -> String {
    let mut hasher = Sha256::new();
    for part in [
        seq.to_string().as_str(),
        at.to_string().as_str(),
        actor,
        action,
        category,
        server_id,
        target,
        result,
        detail,
        meta,
        ip,
        request_id,
        prev_hash,
    ] {
        hasher.update(part.as_bytes());
        hasher.update([0x1f]);
    }
    hex::encode(hasher.finalize())
}

/// Chain hashing for the host recovery command, which runs without an
/// `AppState`. Field order matches [`compute_hash_raw`] exactly, so a record
/// written this way verifies like any other.
#[allow(clippy::too_many_arguments)]
pub fn hash_for_cli(
    seq: i64,
    at: i64,
    actor: &str,
    action: &str,
    category: &str,
    result: &str,
    detail: &str,
    request_id: &str,
    prev_hash: &str,
) -> String {
    compute_hash_raw(
        seq, at, actor, action, category, "", "", result, detail, "", "", request_id, prev_hash,
    )
}

/// The current chain head, for writing an off-machine checkpoint.
///
/// Storing this single value somewhere the server cannot reach — object
/// storage, another host — is what turns "tamper-evident locally" into
/// "tamper-evident even against someone who owns this machine".
pub async fn checkpoint(state: &AppState) -> AppResult<Option<(i64, String)>> {
    let row = sqlx::query("SELECT seq, hash FROM audit_events ORDER BY seq DESC LIMIT 1")
        .fetch_optional(&state.db)
        .await?;
    Ok(row.map(|r| (r.get("seq"), r.get("hash"))))
}
