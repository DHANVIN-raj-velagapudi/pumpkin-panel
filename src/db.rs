// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
use crate::error::AppResult;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::path::Path;

pub const SCHEMA: &str = include_str!("schema.sql");

pub async fn connect(path: &Path) -> AppResult<SqlitePool> {
    // Set the filename directly rather than building a `sqlite://` URL: Windows
    // paths contain backslashes and often spaces, which a URL would mangle.
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(5));

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(opts)
        .await?;

    sqlx::raw_sql(SCHEMA).execute(&pool).await?;
    migrate(&pool).await?;
    Ok(pool)
}

/// Columns added after the first release.
///
/// SQLite has no `ADD COLUMN IF NOT EXISTS`, so each statement runs on its own
/// and a "duplicate column" complaint is treated as "already applied".
const MIGRATIONS: &[&str] = &[
    "ALTER TABLE servers ADD COLUMN memory_limit_mb INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE servers ADD COLUMN disk_limit_mb INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE servers ADD COLUMN cpu_cores INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE sessions ADD COLUMN last_seen INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE users ADD COLUMN totp_secret TEXT",
    "ALTER TABLE users ADD COLUMN totp_enabled INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE servers ADD COLUMN backup_schedule TEXT NOT NULL DEFAULT 'off'",
    "ALTER TABLE servers ADD COLUMN backup_keep INTEGER NOT NULL DEFAULT 7",
];

async fn migrate(pool: &SqlitePool) -> AppResult<()> {
    for statement in MIGRATIONS {
        if let Err(e) = sqlx::query(statement).execute(pool).await {
            let message = e.to_string();
            if !message.contains("duplicate column") {
                return Err(e.into());
            }
        }
    }
    Ok(())
}

/// Seconds since the Unix epoch. The panel stores all timestamps this way.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
