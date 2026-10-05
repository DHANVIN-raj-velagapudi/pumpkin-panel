// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
//! Snapshots, retention and restore.
//!
//! A backup is a zip of everything under the server folder except the server
//! program itself: the world, configs, plugins and player data are what cannot
//! be replaced, and the executable is a 100 MB download away.
//!
//! Archives are written into the panel's own data directory rather than beside
//! the world. Keeping them out of the server folder means the file manager
//! cannot reach them, and a restore cannot accidentally nest a backup inside
//! the thing it is backing up.

use crate::db::now;
use crate::error::{AppError, AppResult};
use serde::Serialize;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Files that never belong in a snapshot.
fn is_excluded(path: &Path, binary: &Path) -> bool {
    if path == binary {
        return true;
    }
    match path.file_name().and_then(|n| n.to_str()) {
        // Half-written files from an interrupted save.
        Some(name) => name.ends_with(".panel-tmp"),
        None => false,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BackupRecord {
    pub id: String,
    pub server_id: String,
    pub created_at: i64,
    pub size_bytes: i64,
    pub file_count: i64,
    pub kind: String,
    pub note: Option<String>,
}

/// Where a server's snapshots live.
pub fn backup_dir(data_dir: &Path, server_id: &str) -> PathBuf {
    data_dir.join("backups").join(server_id)
}

pub struct SnapshotResult {
    pub size_bytes: u64,
    pub file_count: i64,
}

/// Packs the server folder into `destination`.
///
/// Runs synchronously and is expected to be called from `spawn_blocking`.
pub fn create_snapshot(
    working_dir: &Path,
    binary: &Path,
    destination: &Path,
) -> Result<SnapshotResult, String> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let root = working_dir
        .canonicalize()
        .map_err(|e| format!("server folder is unreadable: {e}"))?;
    let binary = binary.canonicalize().unwrap_or_else(|_| binary.to_path_buf());

    let file = std::fs::File::create(destination).map_err(|e| e.to_string())?;
    // Written straight to disk, so a large world never has to fit in memory.
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let mut file_count = 0i64;
    let mut buffer = vec![0u8; 64 * 1024];

    for entry in walkdir::WalkDir::new(&root).follow_links(false) {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();

        let Ok(relative) = path.strip_prefix(&root) else {
            continue;
        };
        if relative.as_os_str().is_empty() {
            continue;
        }
        let name = relative.to_string_lossy().replace('\\', "/");

        if entry.file_type().is_dir() {
            let _ = zip.add_directory(name, options);
            continue;
        }
        if !entry.file_type().is_file() || is_excluded(path, &binary) {
            continue;
        }

        zip.start_file(name, options).map_err(|e| e.to_string())?;
        let mut source = std::fs::File::open(path).map_err(|e| e.to_string())?;
        loop {
            let read = source.read(&mut buffer).map_err(|e| e.to_string())?;
            if read == 0 {
                break;
            }
            zip.write_all(&buffer[..read]).map_err(|e| e.to_string())?;
        }
        file_count += 1;
    }

    zip.finish().map_err(|e| e.to_string())?;

    let size_bytes = std::fs::metadata(destination).map(|m| m.len()).unwrap_or(0);
    Ok(SnapshotResult {
        size_bytes,
        file_count,
    })
}

/// Unpacks a snapshot back over the server folder.
///
/// Entry paths are validated the same way uploads are: anything absolute or
/// containing `..` is refused rather than written, so a tampered archive cannot
/// escape the server folder during a restore.
pub fn restore_snapshot(archive_path: &Path, working_dir: &Path) -> Result<i64, String> {
    let root = working_dir
        .canonicalize()
        .map_err(|e| format!("server folder is unreadable: {e}"))?;

    let file = std::fs::File::open(archive_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("unreadable backup: {e}"))?;

    let mut restored = 0i64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;

        let Some(safe) = entry.enclosed_name() else {
            return Err(format!("backup contains an unsafe path: {}", entry.name()));
        };
        let target = root.join(&safe);

        // Belt and braces: confirm the joined path really is under the root.
        if !target.starts_with(&root) {
            return Err(format!("backup entry escapes the server folder: {}", entry.name()));
        }

        if entry.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            continue;
        }

        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        let mut out = std::fs::File::create(&target).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        restored += 1;
    }

    Ok(restored)
}

/// Deletes the oldest automatic snapshots beyond `keep`.
///
/// Manual snapshots are never pruned: someone took those deliberately, usually
/// right before doing something risky.
pub async fn apply_retention(
    db: &sqlx::SqlitePool,
    data_dir: &Path,
    server_id: &str,
    keep: i64,
) -> AppResult<usize> {
    use sqlx::Row;

    if keep <= 0 {
        return Ok(0);
    }

    let rows = sqlx::query(
        "SELECT id FROM backups
         WHERE server_id = ?1 AND kind = 'scheduled'
         ORDER BY created_at DESC
         LIMIT -1 OFFSET ?2",
    )
    .bind(server_id)
    .bind(keep)
    .fetch_all(db)
    .await?;

    let mut removed = 0;
    for row in rows {
        let id: String = row.get("id");
        let path = backup_dir(data_dir, server_id).join(format!("{id}.zip"));
        let _ = tokio::fs::remove_file(&path).await;
        sqlx::query("DELETE FROM backups WHERE id = ?1")
            .bind(&id)
            .execute(db)
            .await?;
        removed += 1;
    }

    Ok(removed)
}

/// How often a server should be snapshotted automatically.
pub fn schedule_interval_secs(schedule: &str) -> Option<i64> {
    match schedule {
        "hourly" => Some(60 * 60),
        "daily" => Some(60 * 60 * 24),
        "weekly" => Some(60 * 60 * 24 * 7),
        _ => None,
    }
}

/// True when a scheduled snapshot is due.
pub fn is_due(schedule: &str, last_at: Option<i64>) -> bool {
    let Some(interval) = schedule_interval_secs(schedule) else {
        return false;
    };
    match last_at {
        // A server that has never been snapshotted gets one on the next sweep.
        None => true,
        Some(last) => now() - last >= interval,
    }
}

pub fn parse_schedule(value: &str) -> AppResult<String> {
    match value {
        "off" | "hourly" | "daily" | "weekly" => Ok(value.to_string()),
        _ => Err(AppError::BadRequest(
            "schedule must be off, hourly, daily or weekly".into(),
        )),
    }
}
