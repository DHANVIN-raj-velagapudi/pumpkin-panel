use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::AppState;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use serde::{Deserialize, Serialize};
use sqlx::Row;

/// Sessions live for two weeks unless revoked.
pub const SESSION_TTL_SECS: i64 = 60 * 60 * 24 * 14;
/// A session also dies after this long without being used, so an abandoned
/// browser on a shared machine does not stay signed in for a fortnight.
pub const SESSION_IDLE_SECS: i64 = 60 * 60 * 24;
/// `last_seen` is only rewritten this often, to avoid a database write on
/// every single request.
const LAST_SEEN_REFRESH_SECS: i64 = 60;
pub const COOKIE_NAME: &str = "pp_session";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    User,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::User => "user",
        }
    }

    pub fn parse(s: &str) -> Self {
        if s == "admin" {
            Self::Admin
        } else {
            Self::User
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CurrentUser {
    pub id: String,
    pub username: String,
    pub email: Option<String>,
    pub role: Role,
}

impl CurrentUser {
    pub fn is_admin(&self) -> bool {
        self.role == Role::Admin
    }

    pub fn require_admin(&self) -> AppResult<()> {
        if self.is_admin() {
            Ok(())
        } else {
            Err(AppError::Forbidden)
        }
    }
}

/// What a given user may do with a given server.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ServerAccess {
    pub console: bool,
    pub power: bool,
    pub files: bool,
    pub config: bool,
}

impl ServerAccess {
    pub const fn full() -> Self {
        Self { console: true, power: true, files: true, config: true }
    }
}

pub fn hash_password(password: &str) -> AppResult<String> {
    let salt = SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AppError::Other(anyhow::anyhow!("failed to hash password: {e}")))
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

pub fn new_session_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Reads a named cookie out of the raw `Cookie` header.
fn cookie_value<'a>(parts: &'a Parts, name: &str) -> Option<&'a str> {
    let header = parts.headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    header.split(';').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k.trim() == name).then_some(v.trim())
    })
}

pub async fn user_for_token(state: &AppState, token: &str) -> AppResult<Option<CurrentUser>> {
    let moment = now();
    let row = sqlx::query(
        "SELECT u.id, u.username, u.email, u.role, s.last_seen
         FROM sessions s
         JOIN users u ON u.id = s.user_id
         WHERE s.token = ?1 AND s.expires_at > ?2 AND u.is_active = 1",
    )
    .bind(token)
    .bind(moment)
    .fetch_optional(&state.db)
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };

    // Sessions created before this column existed have last_seen = 0; treat
    // those as fresh rather than locking existing users out on upgrade.
    let last_seen: i64 = row.get("last_seen");
    if last_seen > 0 && moment - last_seen > SESSION_IDLE_SECS {
        sqlx::query("DELETE FROM sessions WHERE token = ?1")
            .bind(token)
            .execute(&state.db)
            .await?;
        return Ok(None);
    }

    if moment - last_seen > LAST_SEEN_REFRESH_SECS {
        let _ = sqlx::query("UPDATE sessions SET last_seen = ?1 WHERE token = ?2")
            .bind(moment)
            .bind(token)
            .execute(&state.db)
            .await;
    }

    Ok(Some(CurrentUser {
        id: row.get("id"),
        username: row.get("username"),
        email: row.get("email"),
        role: Role::parse(row.get::<String, _>("role").as_str()),
    }))
}

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = cookie_value(parts, COOKIE_NAME)
            .map(str::to_owned)
            .ok_or(AppError::Unauthorized)?;
        user_for_token(state, &token)
            .await?
            .ok_or(AppError::Unauthorized)
    }
}

/// The raw session token from the request, for handlers that need to act on
/// this one device rather than the whole account.
pub struct SessionToken(pub String);

impl FromRequestParts<AppState> for SessionToken {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        cookie_value(parts, COOKIE_NAME)
            .map(|token| Self(token.to_owned()))
            .ok_or(AppError::Unauthorized)
    }
}

/// Resolves a user's access to one server. Admins always have full access.
pub async fn server_access(
    state: &AppState,
    user: &CurrentUser,
    server_id: &str,
) -> AppResult<ServerAccess> {
    if user.is_admin() {
        return Ok(ServerAccess::full());
    }

    let row = sqlx::query(
        "SELECT can_console, can_power, can_files, can_config
         FROM server_permissions WHERE user_id = ?1 AND server_id = ?2",
    )
    .bind(&user.id)
    .bind(server_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::Forbidden)?;

    Ok(ServerAccess {
        console: row.get::<i64, _>("can_console") != 0,
        power: row.get::<i64, _>("can_power") != 0,
        files: row.get::<i64, _>("can_files") != 0,
        config: row.get::<i64, _>("can_config") != 0,
    })
}

/// Compatibility wrapper over [`crate::audit::record`].
///
/// Existing call sites pass a dotted action such as `files.write`; the category
/// is derived from its prefix so every one of them lands in the chained log
/// without needing to be rewritten.
pub async fn audit(
    state: &AppState,
    user: Option<&CurrentUser>,
    server_id: Option<&str>,
    action: &str,
    detail: Option<&str>,
) {
    use crate::audit::{Category, Event, Outcome};

    let category = match action.split('.').next().unwrap_or("") {
        "auth" => Category::Security,
        "user" => Category::Users,
        "server" | "console" => Category::Servers,
        "files" => Category::Files,
        "config" => Category::Config,
        "player" => Category::Players,
        "backup" => Category::Backups,
        _ => Category::Security,
    };

    let mut event = Event::new(action, category, Outcome::Success);
    if let Some(user) = user {
        event = event.actor(user);
    }
    if let Some(id) = server_id {
        event = event.server(id);
    }
    if let Some(detail) = detail {
        event = event.detail(detail).target(detail);
    }

    crate::audit::record(state, event).await;
}
