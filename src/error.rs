// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Every fallible handler in the panel returns this.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("authentication required")]
    Unauthorized,
    #[error("you do not have permission to do that")]
    Forbidden,
    #[error("{0} not found")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    TooManyRequests(String),
    /// The password was right but a second factor is still needed.
    #[error("a two-factor code is required")]
    MfaRequired,
    #[error("{0}")]
    PayloadTooLarge(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl AppError {
    fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::TooManyRequests(_) => StatusCode::TOO_MANY_REQUESTS,
            Self::MfaRequired => StatusCode::UNAUTHORIZED,
            Self::PayloadTooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Database(_) | Self::Io(_) | Self::Other(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // The sign-in screen needs to tell "wrong password" apart from
        // "password fine, now give me your code", so this one carries a flag.
        if matches!(self, Self::MfaRequired) {
            return (
                StatusCode::UNAUTHORIZED,
                axum::Json(json!({
                    "error": self.to_string(),
                    "mfa_required": true,
                })),
            )
                .into_response();
        }

        let status = self.status();
        // Internal faults are logged in full but reported opaquely.
        let message = if status == StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!(error = %self, "internal error");
            "internal server error".to_string()
        } else {
            self.to_string()
        };
        (status, axum::Json(json!({ "error": message }))).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
