// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
//! Two-factor authentication.
//!
//! TOTP is the second factor: the panel stores a shared secret, the user's
//! authenticator app derives a six-digit code from it and the clock, and both
//! sides arrive at the same number without anything crossing the network.
//!
//! Recovery codes are the way back in when the phone is lost. They are single
//! use and stored as digests, so a copy of the database does not hand anyone a
//! working set.

use crate::audit::{Category, Event, Outcome};
use crate::auth::{verify_password, CurrentUser};
use crate::error::{AppError, AppResult};
use crate::AppState;
use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::Row;
use totp_rs::{Algorithm, Builder, Secret, Totp};

const ISSUER: &str = "Pumpkin Panel";
const RECOVERY_CODE_COUNT: usize = 10;

/// Recovery codes are high-entropy random strings rather than user-chosen
/// secrets, so a single SHA-256 is the right tool; Argon2's slowness exists to
/// frustrate dictionary attacks that do not apply here, and would mean ten slow
/// hashes on every sign-in attempt.
fn hash_code(code: &str) -> String {
    hex::encode(Sha256::digest(code.replace('-', "").to_uppercase().as_bytes()))
}

/// Formats as `A7K9-X2PM`, which is easy to read off paper.
fn generate_recovery_code() -> String {
    // Ambiguous characters are left out so 0/O and 1/I cannot be mistyped.
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let raw = uuid::Uuid::new_v4();
    let bytes = raw.as_bytes();
    let letters: String = bytes
        .iter()
        .take(8)
        .map(|b| ALPHABET[(*b as usize) % ALPHABET.len()] as char)
        .collect();
    format!("{}-{}", &letters[..4], &letters[4..])
}

/// Secrets are held as hex so they survive a round trip through SQLite
/// unambiguously; the base32 form is only produced for the enrolment screen,
/// because that is what authenticator apps expect to be shown.
fn build_totp(secret_hex: &str, username: &str) -> AppResult<Totp> {
    let bytes = hex::decode(secret_hex)
        .map_err(|_| AppError::Other(anyhow::anyhow!("stored TOTP secret is unreadable")))?;

    Builder::new()
        .with_algorithm(Algorithm::SHA1)
        .with_digits(6)
        // One step of tolerance either way, so a slightly wrong clock still works.
        .with_skew(1)
        .with_step_duration(30)
        .with_secret(bytes)
        .with_issuer(Some(ISSUER))
        .with_account_name(username)
        .build()
        .map_err(|e| AppError::Other(anyhow::anyhow!("could not build TOTP: {e}")))
}

/// Checks a six-digit code against the user's secret.
pub async fn verify_totp(state: &AppState, user_id: &str, code: &str) -> AppResult<bool> {
    let row = sqlx::query("SELECT username, totp_secret FROM users WHERE id = ?1")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await?;

    let Some(row) = row else { return Ok(false) };
    let Some(stored) = row.get::<Option<String>, _>("totp_secret") else {
        return Ok(false);
    };
    let secret = crate::crypto::decrypt(&state.secret_key, &stored)?;

    let totp = build_totp(&secret, &row.get::<String, _>("username"))?;
    // Returns the matching time step, so "some" means the code was valid.
    Ok(totp.check_current(code.trim()).is_some())
}

/// Spends a recovery code. Returns true when one matched and was unused.
pub async fn consume_recovery_code(
    state: &AppState,
    user_id: &str,
    code: &str,
) -> AppResult<bool> {
    let hash = hash_code(code);
    let result = sqlx::query(
        "UPDATE recovery_codes SET used_at = ?1
         WHERE user_id = ?2 AND code_hash = ?3 AND used_at IS NULL",
    )
    .bind(crate::db::now())
    .bind(user_id)
    .bind(&hash)
    .execute(&state.db)
    .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn is_enabled(state: &AppState, user_id: &str) -> AppResult<bool> {
    let row = sqlx::query("SELECT totp_enabled FROM users WHERE id = ?1")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await?;
    Ok(row.map_or(false, |r| r.get::<i64, _>("totp_enabled") != 0))
}

/// Step one: generate a secret and show it as a QR code. Nothing is enforced
/// until the user proves they can produce a code from it.
pub async fn setup(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<serde_json::Value>> {
    if is_enabled(&state, &user.id).await? {
        return Err(AppError::Conflict(
            "two-factor authentication is already switched on".into(),
        ));
    }

    // 160 bits, the size RFC 4226 recommends and every authenticator supports.
    let mut raw = [0u8; 20];
    argon2::password_hash::rand_core::RngCore::fill_bytes(
        &mut argon2::password_hash::rand_core::OsRng,
        &mut raw,
    );
    let encoded = hex::encode(raw);
    let totp = build_totp(&encoded, &user.username)?;
    let base32 = Secret::from(raw.to_vec()).to_base32();

    // Held against the account but not yet active, so a half-finished setup
    // cannot lock anyone out. Encrypted, because unlike a password this value
    // has to be readable again to derive the expected code.
    let sealed = crate::crypto::encrypt(&state.secret_key, &encoded)?;
    sqlx::query("UPDATE users SET totp_secret = ?1, totp_enabled = 0 WHERE id = ?2")
        .bind(&sealed)
        .bind(&user.id)
        .execute(&state.db)
        .await?;

    let qr = totp
        .to_qr_base64()
        .map_err(|e| AppError::Other(anyhow::anyhow!("could not draw the QR code: {e}")))?;
    let uri = totp
        .to_url()
        .map_err(|e| AppError::Other(anyhow::anyhow!("could not build the setup link: {e}")))?;

    Ok(Json(json!({
        // The base32 form is for typing in by hand when a camera is not handy.
        "secret": base32,
        "uri": uri,
        "qr": format!("data:image/png;base64,{qr}"),
    })))
}

#[derive(Debug, Deserialize)]
pub struct CodeRequest {
    pub code: String,
}

/// Step two: confirm a code, switch it on, and hand back the recovery codes.
pub async fn enable(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<CodeRequest>,
) -> AppResult<Json<serde_json::Value>> {
    if !verify_totp(&state, &user.id, &body.code).await? {
        crate::audit::record(
            &state,
            Event::new("MFA_ENABLE_FAILED", Category::Security, Outcome::Failure)
                .actor(&user)
                .detail("code did not match"),
        )
        .await;
        return Err(AppError::BadRequest(
            "that code is not right — check your authenticator and try again".into(),
        ));
    }

    sqlx::query("UPDATE users SET totp_enabled = 1 WHERE id = ?1")
        .bind(&user.id)
        .execute(&state.db)
        .await?;

    // Any codes from a previous enrolment are void.
    sqlx::query("DELETE FROM recovery_codes WHERE user_id = ?1")
        .bind(&user.id)
        .execute(&state.db)
        .await?;

    let mut codes = Vec::with_capacity(RECOVERY_CODE_COUNT);
    for _ in 0..RECOVERY_CODE_COUNT {
        let code = generate_recovery_code();
        sqlx::query("INSERT INTO recovery_codes (user_id, code_hash, used_at) VALUES (?1, ?2, NULL)")
            .bind(&user.id)
            .bind(hash_code(&code))
            .execute(&state.db)
            .await?;
        codes.push(code);
    }

    crate::audit::record(
        &state,
        Event::new("MFA_ENABLED", Category::Security, Outcome::Success)
            .actor(&user)
            .detail(format!("{RECOVERY_CODE_COUNT} recovery codes issued")),
    )
    .await;

    Ok(Json(json!({
        "ok": true,
        "recovery_codes": codes,
        "note": "These are shown once. Store them somewhere safe — each works a single time.",
    })))
}

#[derive(Debug, Deserialize)]
pub struct DisableRequest {
    /// The account password, to prove it is really the owner.
    pub password: String,
}

pub async fn disable(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<DisableRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let hash: String = sqlx::query("SELECT password_hash FROM users WHERE id = ?1")
        .bind(&user.id)
        .fetch_one(&state.db)
        .await?
        .get("password_hash");

    if !verify_password(&body.password, &hash) {
        crate::audit::record(
            &state,
            Event::new("MFA_DISABLE_FAILED", Category::Security, Outcome::Denied)
                .actor(&user)
                .detail("wrong password"),
        )
        .await;
        return Err(AppError::Forbidden);
    }

    sqlx::query("UPDATE users SET totp_enabled = 0, totp_secret = NULL WHERE id = ?1")
        .bind(&user.id)
        .execute(&state.db)
        .await?;
    sqlx::query("DELETE FROM recovery_codes WHERE user_id = ?1")
        .bind(&user.id)
        .execute(&state.db)
        .await?;

    crate::audit::record(
        &state,
        Event::new("MFA_DISABLED", Category::Security, Outcome::Success).actor(&user),
    )
    .await;

    Ok(Json(json!({ "ok": true })))
}

/// How many unused recovery codes remain, for the account screen.
pub async fn status(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<serde_json::Value>> {
    let enabled = is_enabled(&state, &user.id).await?;
    let remaining: i64 = sqlx::query(
        "SELECT COUNT(*) AS n FROM recovery_codes WHERE user_id = ?1 AND used_at IS NULL",
    )
    .bind(&user.id)
    .fetch_one(&state.db)
    .await?
    .get("n");

    Ok(Json(json!({
        "enabled": enabled,
        "recovery_codes_remaining": remaining,
    })))
}
