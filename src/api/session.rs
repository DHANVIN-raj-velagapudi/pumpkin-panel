// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
use crate::auth::{
    audit, hash_password, new_session_token, verify_password, CurrentUser, Role, SessionToken,
    COOKIE_NAME, SESSION_TTL_SECS,
};
use crate::audit::{Category, Event, Outcome};
use crate::db::now;
use crate::error::{AppError, AppResult};
use crate::AppState;
use axum::extract::{ConnectInfo, State};
use axum::http::header::{HeaderValue, SET_COOKIE};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use sqlx::Row;
use std::net::SocketAddr;

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    /// TOTP code, or a recovery code, when the account has a second factor.
    #[serde(default)]
    pub code: Option<String>,
}

fn session_cookie(token: &str, max_age: i64, secure: bool) -> HeaderValue {
    // `Secure` is only set when the panel is actually reachable over HTTPS;
    // setting it on plain HTTP would silently break sign-in.
    let secure_flag = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax{secure_flag}; Max-Age={max_age}"
    ))
    .unwrap_or_else(|_| HeaderValue::from_static(""))
}

pub async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<LoginRequest>,
) -> AppResult<Response> {
    let username = body.username.trim().to_ascii_lowercase();
    // Keyed on address and account together, so an attacker hammering one
    // username cannot lock its real owner out from a different machine.
    let throttle_key = format!("{}|{}", peer.ip(), username);

    if let Some(seconds) = state.throttle.locked_for(&throttle_key) {
        tracing::warn!(client = %peer.ip(), user = %username, "sign-in locked out");
        crate::audit::record(
            &state,
            Event::new("LOGIN_BLOCKED", Category::Security, Outcome::Denied)
                .actor_name(&username)
                .ip(Some(peer.ip().to_string()))
                .detail(format!("locked out for another {seconds}s")),
        )
        .await;
        return Err(AppError::TooManyRequests(format!(
            "too many failed sign-ins, try again in {seconds} seconds"
        )));
    }

    let row = sqlx::query(
        "SELECT id, username, email, role, password_hash, is_active
         FROM users WHERE username = ?1",
    )
    .bind(body.username.trim())
    .fetch_optional(&state.db)
    .await?;

    // Same error for unknown user and wrong password, so the response does not
    // reveal which usernames exist.
    let Some(row) = row else {
        state.throttle.record_failure(&throttle_key);
        crate::audit::record(
            &state,
            Event::new("LOGIN_FAILED", Category::Security, Outcome::Failure)
                .actor_name(&username)
                .ip(Some(peer.ip().to_string()))
                .detail("no such account"),
        )
        .await;
        return Err(AppError::BadRequest("invalid username or password".into()));
    };
    let hash: String = row.get("password_hash");

    if !verify_password(&body.password, &hash) {
        state.throttle.record_failure(&throttle_key);
        tracing::warn!(client = %peer.ip(), user = %username, "failed sign-in");
        crate::audit::record(
            &state,
            Event::new("LOGIN_FAILED", Category::Security, Outcome::Failure)
                .actor_name(&username)
                .ip(Some(peer.ip().to_string()))
                .detail("wrong password"),
        )
        .await;
        return Err(AppError::BadRequest("invalid username or password".into()));
    }
    if row.get::<i64, _>("is_active") == 0 {
        state.throttle.record_failure(&throttle_key);
        return Err(AppError::Forbidden);
    }

    let user = CurrentUser {
        id: row.get("id"),
        username: row.get("username"),
        email: row.get("email"),
        role: Role::parse(row.get::<String, _>("role").as_str()),
    };

    // The password was right; if a second factor is configured it has to be
    // satisfied before a session is issued.
    if crate::api::mfa::is_enabled(&state, &user.id).await? {
        let Some(code) = body.code.as_deref().map(str::trim).filter(|c| !c.is_empty()) else {
            // Not counted as a failure: the user has not got it wrong yet.
            return Err(AppError::MfaRequired);
        };

        let by_totp = crate::api::mfa::verify_totp(&state, &user.id, code).await?;
        let by_recovery = if by_totp {
            false
        } else {
            crate::api::mfa::consume_recovery_code(&state, &user.id, code).await?
        };

        if !by_totp && !by_recovery {
            state.throttle.record_failure(&throttle_key);
            crate::audit::record(
                &state,
                Event::new("MFA_FAILED", Category::Security, Outcome::Failure)
                    .actor(&user)
                    .ip(Some(peer.ip().to_string()))
                    .detail("second factor did not match"),
            )
            .await;
            return Err(AppError::BadRequest("that code is not right".into()));
        }

        if by_recovery {
            crate::audit::record(
                &state,
                Event::new("RECOVERY_CODE_USED", Category::Security, Outcome::Success)
                    .actor(&user)
                    .ip(Some(peer.ip().to_string()))
                    .detail("signed in with a recovery code"),
            )
            .await;
        }
    }

    state.throttle.record_success(&throttle_key);

    let token = new_session_token();
    let issued = now();
    sqlx::query(
        "INSERT INTO sessions (token, user_id, created_at, expires_at, last_seen)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(&token)
    .bind(&user.id)
    .bind(issued)
    .bind(issued + SESSION_TTL_SECS)
    .bind(issued)
    .execute(&state.db)
    .await?;

    // Opportunistically clear expired sessions.
    let _ = sqlx::query("DELETE FROM sessions WHERE expires_at <= ?1")
        .bind(issued)
        .execute(&state.db)
        .await;

    audit(
        &state,
        Some(&user),
        None,
        "auth.login",
        Some(&peer.ip().to_string()),
    )
    .await;

    let mut response = Json(json!({ "user": user })).into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        session_cookie(&token, SESSION_TTL_SECS, state.secure_cookies),
    );
    Ok(response)
}

pub async fn logout(
    State(state): State<AppState>,
    user: CurrentUser,
    jar: SessionToken,
) -> AppResult<Response> {
    // Only this device is signed out; other sessions keep working.
    sqlx::query("DELETE FROM sessions WHERE token = ?1")
        .bind(&jar.0)
        .execute(&state.db)
        .await?;

    audit(&state, Some(&user), None, "auth.logout", None).await;

    let mut response = Json(json!({ "ok": true })).into_response();
    response
        .headers_mut()
        .insert(SET_COOKIE, session_cookie("", 0, state.secure_cookies));
    Ok(response)
}

/// Revokes every session for the signed-in account.
pub async fn logout_everywhere(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Response> {
    sqlx::query("DELETE FROM sessions WHERE user_id = ?1")
        .bind(&user.id)
        .execute(&state.db)
        .await?;

    audit(&state, Some(&user), None, "auth.logout_all", None).await;

    let mut response = Json(json!({ "ok": true })).into_response();
    response
        .headers_mut()
        .insert(SET_COOKIE, session_cookie("", 0, state.secure_cookies));
    Ok(response)
}

pub async fn me(user: CurrentUser) -> Json<serde_json::Value> {
    Json(json!({ "user": user }))
}

/// Creates the first administrator when the users table is empty.
pub async fn ensure_bootstrap_admin(
    state: &AppState,
    username: &str,
    password: &str,
) -> AppResult<bool> {
    let count: i64 = sqlx::query("SELECT COUNT(*) AS n FROM users")
        .fetch_one(&state.db)
        .await?
        .get("n");

    if count > 0 {
        return Ok(false);
    }

    sqlx::query(
        "INSERT INTO users (id, username, email, password_hash, role, is_active, created_at)
         VALUES (?1, ?2, NULL, ?3, 'admin', 1, ?4)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(username)
    .bind(hash_password(password)?)
    .bind(now())
    .execute(&state.db)
    .await?;

    Ok(true)
}
