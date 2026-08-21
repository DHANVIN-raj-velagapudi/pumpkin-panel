use crate::api::load_server;
use crate::auth::{audit, server_access, CurrentUser};
use crate::error::{AppError, AppResult};
use crate::security;
use crate::AppState;
use axum::extract::{Path as AxumPath, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Component, Path, PathBuf};

/// Files larger than this are not opened in the editor.
const MAX_EDIT_BYTES: u64 = 4 * 1024 * 1024;

/// Resolves a client-supplied relative path against a server root.
///
/// Rejects absolute paths and `..` traversal, and additionally canonicalizes so
/// that a symlink inside the root cannot be used to escape it.
fn resolve(root: &Path, relative: &str) -> AppResult<PathBuf> {
    let root = root
        .canonicalize()
        .map_err(|e| AppError::BadRequest(format!("server directory is unreadable: {e}")))?;

    let requested = PathBuf::from(relative.trim().trim_start_matches(['/', '\\']));
    let mut safe = root.clone();

    for component in requested.components() {
        match component {
            Component::Normal(part) => safe.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(AppError::BadRequest("path is not allowed".into()));
            }
        }
    }

    // An existing path must really live under the root once symlinks are followed.
    // For a path being created, the same must hold for its parent.
    let check = if safe.exists() {
        safe.canonicalize().ok()
    } else {
        safe.parent().and_then(|p| p.canonicalize().ok())
    };

    match check {
        Some(resolved) if resolved.starts_with(&root) => Ok(safe),
        Some(_) => Err(AppError::BadRequest("path escapes the server directory".into())),
        None => Err(AppError::NotFound("path".into())),
    }
}

/// Loads a server and checks that the caller may touch its files.
async fn root_for(state: &AppState, user: &CurrentUser, id: &str) -> AppResult<PathBuf> {
    let access = server_access(state, user, id).await?;
    if !access.files {
        return Err(AppError::Forbidden);
    }
    let server = load_server(state, id).await?;
    Ok(PathBuf::from(server.working_dir))
}

/// The server executable is the one file in the directory that must never be
/// writable through the panel.
///
/// The file manager otherwise hands out write access to everything under the
/// server folder, and the panel launches this exact path on Start — so being
/// able to replace it would turn "can edit files" into "can run any code on
/// this machine". Reading and downloading it stay allowed.
async fn deny_if_protected(
    state: &AppState,
    server_id: &str,
    target: &Path,
) -> AppResult<()> {
    let server = load_server(state, server_id).await?;
    let binary = PathBuf::from(&server.binary_path);

    // Compare canonical paths so a different spelling of the same file, or a
    // symlink pointing at it, is caught too.
    let same = match (target.canonicalize(), binary.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        // A target that does not exist yet cannot be the running binary.
        _ => target == binary,
    };

    if same {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct PathQuery {
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: i64,
}

fn modified_secs(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64)
}

fn join_rel(base: &str, name: &str) -> String {
    if base.is_empty() {
        name.to_string()
    } else {
        format!("{}/{}", base.trim_end_matches('/'), name)
    }
}

pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<PathQuery>,
) -> AppResult<Json<serde_json::Value>> {
    let root = root_for(&state, &user, &id).await?;
    let dir = resolve(&root, &query.path)?;

    if !dir.is_dir() {
        return Err(AppError::BadRequest("not a directory".into()));
    }

    let mut entries = Vec::new();
    let mut reader = tokio::fs::read_dir(&dir).await?;
    while let Some(item) = reader.next_entry().await? {
        let meta = match item.metadata().await {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        let name = item.file_name().to_string_lossy().to_string();
        entries.push(Entry {
            path: join_rel(query.path.trim_matches('/'), &name),
            name,
            is_dir: meta.is_dir(),
            size: meta.len(),
            modified: modified_secs(&meta),
        });
    }

    // Directories first, then alphabetical.
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(Json(json!({ "path": query.path, "entries": entries })))
}

pub async fn read(
    State(state): State<AppState>,
    user: CurrentUser,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<PathQuery>,
) -> AppResult<Json<serde_json::Value>> {
    let root = root_for(&state, &user, &id).await?;
    let file = resolve(&root, &query.path)?;

    let meta = tokio::fs::metadata(&file).await?;
    if meta.is_dir() {
        return Err(AppError::BadRequest("that is a directory".into()));
    }
    if meta.len() > MAX_EDIT_BYTES {
        return Err(AppError::BadRequest(format!(
            "file is too large to edit ({} bytes)",
            meta.len()
        )));
    }

    let bytes = tokio::fs::read(&file).await?;
    let content = String::from_utf8(bytes)
        .map_err(|_| AppError::BadRequest("file is not valid UTF-8 text".into()))?;

    Ok(Json(json!({
        "path": query.path,
        "content": content,
        "size": meta.len(),
        "modified": modified_secs(&meta),
    })))
}

#[derive(Debug, Deserialize)]
pub struct WriteRequest {
    pub path: String,
    pub content: String,
}

pub async fn write(
    State(state): State<AppState>,
    user: CurrentUser,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<WriteRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let root = root_for(&state, &user, &id).await?;
    let file = resolve(&root, &body.path)?;
    deny_if_protected(&state, &id, &file).await?;

    if file.is_dir() {
        return Err(AppError::BadRequest("that is a directory".into()));
    }

    // The previous contents are hashed before the write so the log can record
    // what changed. The contents themselves are never stored: a digest and a
    // size say what happened without turning the audit log into a second copy
    // of every file on the server.
    let previous = tokio::fs::read(&file).await.ok();
    let old_hash = previous.as_deref().map(crate::audit::hash_bytes);
    let old_size = previous.as_ref().map(|b| b.len());

    // Write beside the target then rename, so a failure cannot truncate the original.
    let temp = file.with_extension("panel-tmp");
    tokio::fs::write(&temp, body.content.as_bytes()).await?;
    tokio::fs::rename(&temp, &file).await?;

    let new_bytes = body.content.as_bytes();
    let new_hash = crate::audit::hash_bytes(new_bytes);
    let unchanged = old_hash.as_deref() == Some(new_hash.as_str());

    crate::audit::record(
        &state,
        crate::audit::Event::new(
            if old_hash.is_some() { "EDIT_FILE" } else { "CREATE_FILE" },
            crate::audit::Category::Files,
            crate::audit::Outcome::Success,
        )
        .actor(&user)
        .server(&id)
        .target(&body.path)
        .detail(match (old_size, unchanged) {
            (_, true) => "saved with no changes".to_string(),
            (Some(old), _) => format!("{old} bytes -> {} bytes", new_bytes.len()),
            (None, _) => format!("created, {} bytes", new_bytes.len()),
        })
        .meta(json!({
            "old_sha256": old_hash,
            "new_sha256": new_hash,
            "old_size": old_size,
            "new_size": new_bytes.len(),
        })),
    )
    .await;

    Ok(Json(json!({ "ok": true })))
}

pub async fn mkdir(
    State(state): State<AppState>,
    user: CurrentUser,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<PathQuery>,
) -> AppResult<Json<serde_json::Value>> {
    let root = root_for(&state, &user, &id).await?;
    let dir = resolve(&root, &body.path)?;

    tokio::fs::create_dir_all(&dir).await?;
    audit(&state, Some(&user), Some(&id), "files.mkdir", Some(&body.path)).await;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
pub struct RenameRequest {
    pub from: String,
    pub to: String,
}

pub async fn rename(
    State(state): State<AppState>,
    user: CurrentUser,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<RenameRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let root = root_for(&state, &user, &id).await?;
    let from = resolve(&root, &body.from)?;
    let to = resolve(&root, &body.to)?;
    deny_if_protected(&state, &id, &from).await?;
    deny_if_protected(&state, &id, &to).await?;

    if to.exists() {
        return Err(AppError::Conflict("target already exists".into()));
    }

    tokio::fs::rename(&from, &to).await?;
    audit(
        &state,
        Some(&user),
        Some(&id),
        "files.rename",
        Some(&format!("{} -> {}", body.from, body.to)),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
pub struct DeleteRequest {
    pub path: String,
    #[serde(default)]
    pub recursive: bool,
}

pub async fn remove(
    State(state): State<AppState>,
    user: CurrentUser,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<DeleteRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let root = root_for(&state, &user, &id).await?;
    let target = resolve(&root, &body.path)?;
    deny_if_protected(&state, &id, &target).await?;

    if target == root.canonicalize()? {
        return Err(AppError::BadRequest(
            "refusing to delete the server directory".into(),
        ));
    }

    if target.is_dir() {
        if body.recursive {
            tokio::fs::remove_dir_all(&target).await?;
        } else {
            tokio::fs::remove_dir(&target).await?;
        }
    } else {
        tokio::fs::remove_file(&target).await?;
    }

    audit(&state, Some(&user), Some(&id), "files.delete", Some(&body.path)).await;
    Ok(Json(json!({ "ok": true })))
}

/// Streams an uploaded file into a directory under the server root.
pub async fn upload(
    State(state): State<AppState>,
    user: CurrentUser,
    AxumPath(id): AxumPath<String>,
    mut multipart: axum::extract::Multipart,
) -> AppResult<Json<serde_json::Value>> {
    let root = root_for(&state, &user, &id).await?;

    let server = load_server(&state, &id).await?;
    let disk_limit_mb = (server.disk_limit_mb > 0).then_some(server.disk_limit_mb as u64);
    let used_bytes = if disk_limit_mb.is_some() {
        let dir = PathBuf::from(&server.working_dir);
        tokio::task::spawn_blocking(move || {
            walkdir::WalkDir::new(dir)
                .follow_links(false)
                .into_iter()
                .filter_map(Result::ok)
                .filter_map(|e| e.metadata().ok())
                .filter(std::fs::Metadata::is_file)
                .map(|m| m.len())
                .sum::<u64>()
        })
        .await
        .unwrap_or(0)
    } else {
        0
    };

    let mut directory = String::new();
    let mut written: Vec<String> = Vec::new();
    let mut mismatches: Vec<String> = Vec::new();
    let mut accepted: u64 = 0;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("upload failed: {e}")))?
    {
        match field.name() {
            Some("path") => {
                directory = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("upload failed: {e}")))?;
            }
            Some("file") => {
                let filename = field
                    .file_name()
                    .map(str::to_owned)
                    .ok_or_else(|| AppError::BadRequest("upload is missing a filename".into()))?;

                // Only the base name is honoured; a path in the filename is a
                // classic way to try to escape the upload directory.
                let base = std::path::Path::new(&filename)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .ok_or_else(|| AppError::BadRequest("invalid filename".into()))?;

                let relative = if directory.trim().is_empty() {
                    base.clone()
                } else {
                    format!("{}/{}", directory.trim().trim_matches('/'), base)
                };
                let target = resolve(&root, &relative)?;
                deny_if_protected(&state, &id, &target).await?;

                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("upload failed: {e}")))?;

                // ---- size ----
                let size = bytes.len() as u64;
                if size > security::MAX_UPLOAD_BYTES {
                    return Err(AppError::PayloadTooLarge(format!(
                        "{base} is {} MB, over the {} MB limit for a single upload",
                        size / 1024 / 1024,
                        security::MAX_UPLOAD_BYTES / 1024 / 1024
                    )));
                }

                // ---- disk allowance ----
                if let Some(limit_mb) = disk_limit_mb {
                    let limit = limit_mb * 1024 * 1024;
                    if used_bytes + accepted + size > limit {
                        return Err(AppError::BadRequest(format!(
                            "uploading {base} would take this server past its {limit_mb} MB disk allowance"
                        )));
                    }
                }

                // ---- malware scan ----
                //
                // Runs before the bytes reach disk, so an infected file is
                // never available to the server even briefly.
                if let Some(address) = &state.clamav {
                    match security::scan_with_clamav(address, &bytes).await {
                        Ok(security::ScanVerdict::Clean) => {}
                        Ok(security::ScanVerdict::Infected(signature)) => {
                            tracing::warn!(file = %base, %signature, "upload rejected by clamd");
                            crate::audit::record(
                                &state,
                                crate::audit::Event::new(
                                    "UPLOAD_BLOCKED",
                                    crate::audit::Category::Files,
                                    crate::audit::Outcome::Denied,
                                )
                                .actor(&user)
                                .server(&id)
                                .target(&base)
                                .detail(format!("malware signature {signature}")),
                            )
                            .await;
                            return Err(AppError::BadRequest(format!(
                                "{base} was rejected by the virus scanner ({signature})"
                            )));
                        }
                        Err(reason) => {
                            // A scanner that cannot be reached must not silently
                            // wave files through.
                            tracing::error!(%reason, "virus scan failed");
                            return Err(AppError::Other(anyhow::anyhow!(
                                "the virus scanner is unavailable, so the upload was refused: {reason}"
                            )));
                        }
                    }
                }

                // ---- archive safety ----
                //
                // The panel never extracts uploads, so a bomb cannot detonate
                // here. It is still rejected on the way in, because the whole
                // point of uploading a plugin archive is that something else
                // will unpack it later.
                if security::is_archive_name(&base) {
                    match security::inspect_zip(&bytes) {
                        Ok(report) => {
                            tracing::info!(
                                file = %base,
                                entries = report.entries,
                                compressed = report.compressed,
                                uncompressed = report.uncompressed,
                                ratio = report.ratio,
                                nested = report.nested_archives,
                                "archive accepted"
                            );
                            if report.nested_archives > 0 {
                                mismatches.push(format!(
                                    "{base} contains {} nested archive(s)",
                                    report.nested_archives
                                ));
                            }
                        }
                        Err(reason) => {
                            tracing::warn!(file = %base, %reason, "archive rejected");
                            return Err(AppError::BadRequest(format!("{base}: {reason}")));
                        }
                    }
                }

                // ---- executables ----
                //
                // Not blocked: dropping a binary into the folder is harmless
                // while the panel only ever launches the configured server
                // program, which `deny_if_protected` keeps read-only. It is
                // surfaced so an operator can see it happened.
                if security::looks_executable(&bytes) {
                    tracing::warn!(file = %base, "uploaded file is a native executable");
                    mismatches.push(format!("{base} is an executable"));
                }

                // ---- content type ----
                //
                // Recorded rather than enforced: a mismatch is worth knowing
                // about, but plenty of legitimate files have odd extensions and
                // refusing them would make the panel annoying to use.
                if let Some(detected) = security::sniff(&bytes) {
                    let claimed = mime_guess::from_path(&base).first_raw().unwrap_or("unknown");
                    if claimed != detected {
                        tracing::info!(
                            file = %base, %claimed, %detected,
                            "upload content does not match its extension"
                        );
                        mismatches.push(format!("{base} ({detected})"));
                    }
                }

                tokio::fs::write(&target, &bytes).await?;
                accepted += size;
                written.push(relative);
            }
            _ => {}
        }
    }

    if written.is_empty() {
        return Err(AppError::BadRequest("no file was supplied".into()));
    }

    audit(
        &state,
        Some(&user),
        Some(&id),
        "files.upload",
        Some(&written.join(", ")),
    )
    .await;

    Ok(Json(json!({
        "ok": true,
        "files": written,
        "warnings": mismatches,
    })))
}

/// Sends a file back to the browser as a download.
pub async fn download(
    State(state): State<AppState>,
    user: CurrentUser,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<PathQuery>,
) -> AppResult<axum::response::Response> {
    use axum::http::header;
    use axum::response::IntoResponse;

    let root = root_for(&state, &user, &id).await?;
    let file = resolve(&root, &query.path)?;

    let meta = tokio::fs::metadata(&file).await?;
    if meta.is_dir() {
        return Err(AppError::BadRequest("that is a directory".into()));
    }

    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());

    let bytes = tokio::fs::read(&file).await?;

    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}\""),
            ),
        ],
        bytes,
    )
        .into_response())
}

/// Upper bound on what one archive may contain, uncompressed.
const MAX_ARCHIVE_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct ArchiveRequest {
    /// Files and folders to include, relative to the server root.
    pub paths: Vec<String>,
}

/// Packs a selection of files and folders into a zip and returns it.
pub async fn archive(
    State(state): State<AppState>,
    user: CurrentUser,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<ArchiveRequest>,
) -> AppResult<axum::response::Response> {
    use axum::http::header;
    use axum::response::IntoResponse;

    let root = root_for(&state, &user, &id).await?;
    if body.paths.is_empty() {
        return Err(AppError::BadRequest("nothing was selected".into()));
    }

    // Every path is validated before any work starts.
    let mut targets = Vec::new();
    for path in &body.paths {
        targets.push(resolve(&root, path)?);
    }

    let canonical_root = root.canonicalize()?;
    let selection = body.paths.join(", ");

    let bytes = tokio::task::spawn_blocking(move || build_archive(&canonical_root, &targets))
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!("archive task failed: {e}")))?
        .map_err(AppError::BadRequest)?;

    // A single file keeps its own name; a mixed selection gets a generic one.
    let name = if body.paths.len() == 1 {
        let stem = std::path::Path::new(&body.paths[0])
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "files".to_string());
        format!("{stem}.zip")
    } else {
        format!("{}-files.zip", body.paths.len())
    };

    audit(&state, Some(&user), Some(&id), "files.archive", Some(&selection)).await;

    Ok((
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}\""),
            ),
        ],
        bytes,
    )
        .into_response())
}

fn build_archive(root: &Path, targets: &[PathBuf]) -> Result<Vec<u8>, String> {
    use std::io::{Read, Write};
    use zip::write::SimpleFileOptions;

    let mut buffer = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(&mut buffer);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut total: u64 = 0;

    // Zip entries always use forward slashes, whatever the host platform does.
    let entry_name = |path: &Path| -> Option<String> {
        path.strip_prefix(root)
            .ok()
            .map(|rel| rel.to_string_lossy().replace('\\', "/"))
    };

    let add_file = |zip: &mut zip::ZipWriter<_>, path: &Path, total: &mut u64| {
        let Some(name) = entry_name(path) else {
            return Ok(());
        };
        let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;

        *total += meta.len();
        if *total > MAX_ARCHIVE_BYTES {
            return Err(format!(
                "selection is larger than {} GB, download it in smaller pieces",
                MAX_ARCHIVE_BYTES / 1024 / 1024 / 1024
            ));
        }

        zip.start_file(name, options).map_err(|e| e.to_string())?;

        // Copied in chunks so a large world file does not double in memory.
        let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let read = file.read(&mut chunk).map_err(|e| e.to_string())?;
            if read == 0 {
                break;
            }
            zip.write_all(&chunk[..read]).map_err(|e| e.to_string())?;
        }
        Ok(())
    };

    for target in targets {
        if target.is_dir() {
            // An empty folder still deserves an entry, so it survives the round trip.
            if let Some(name) = entry_name(target) {
                zip.add_directory(name, options).map_err(|e| e.to_string())?;
            }
            for entry in walkdir::WalkDir::new(target).follow_links(false) {
                let entry = entry.map_err(|e| e.to_string())?;
                if entry.file_type().is_dir() {
                    if let Some(name) = entry_name(entry.path()) {
                        let _ = zip.add_directory(name, options);
                    }
                } else if entry.file_type().is_file() {
                    add_file(&mut zip, entry.path(), &mut total)?;
                }
            }
        } else {
            add_file(&mut zip, target, &mut total)?;
        }
    }

    // `finish` consumes the writer, which releases its borrow of the buffer.
    zip.finish().map_err(|e| e.to_string())?;
    Ok(buffer.into_inner())
}

#[derive(Debug, Deserialize)]
pub struct ExtractRequest {
    /// Archive to unpack, relative to the server root.
    pub path: String,
    /// Folder to unpack into. Defaults to the archive's own folder.
    #[serde(default)]
    pub destination: Option<String>,
}

/// Unpacks a zip that is already stored on the server.
///
/// The archive goes through exactly the same inspection an upload does before a
/// single byte is written, and every entry path is re-validated during
/// extraction. That second check matters: the file could have been placed there
/// by some route other than the upload endpoint.
pub async fn extract(
    State(state): State<AppState>,
    user: CurrentUser,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<ExtractRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let root = root_for(&state, &user, &id).await?;
    let archive = resolve(&root, &body.path)?;

    let destination = match body.destination.as_deref().map(str::trim) {
        Some(dir) if !dir.is_empty() => resolve(&root, dir)?,
        _ => archive
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| root.clone()),
    };

    let bytes = tokio::fs::read(&archive).await?;
    let report = security::inspect_zip(&bytes)
        .map_err(|reason| AppError::BadRequest(format!("{}: {reason}", body.path)))?;

    // Refuse before writing anything if the archive would blow the allowance.
    let server = load_server(&state, &id).await?;
    if server.disk_limit_mb > 0 {
        let limit = server.disk_limit_mb as u64 * 1024 * 1024;
        let dir = PathBuf::from(&server.working_dir);
        let used = tokio::task::spawn_blocking(move || {
            walkdir::WalkDir::new(dir)
                .follow_links(false)
                .into_iter()
                .filter_map(Result::ok)
                .filter_map(|e| e.metadata().ok())
                .filter(std::fs::Metadata::is_file)
                .map(|m| m.len())
                .sum::<u64>()
        })
        .await
        .unwrap_or(0);

        if used + report.uncompressed > limit {
            return Err(AppError::BadRequest(format!(
                "unpacking this would take the server past its {} MB disk allowance",
                server.disk_limit_mb
            )));
        }
    }

    let binary = PathBuf::from(&server.binary_path)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(&server.binary_path));
    let root_for_task = root.canonicalize()?;
    let target_dir = destination.clone();

    let written = tokio::task::spawn_blocking(move || {
        unpack(&bytes, &target_dir, &root_for_task, &binary)
    })
    .await
    .map_err(|e| AppError::Other(anyhow::anyhow!("extraction task failed: {e}")))?
    .map_err(AppError::BadRequest)?;

    crate::audit::record(
        &state,
        crate::audit::Event::new(
            "EXTRACT_ARCHIVE",
            crate::audit::Category::Files,
            crate::audit::Outcome::Success,
        )
        .actor(&user)
        .server(&id)
        .target(&body.path)
        .detail(format!("{written} files unpacked"))
        .meta(json!({
            "entries": report.entries,
            "uncompressed": report.uncompressed,
            "ratio": report.ratio,
        })),
    )
    .await;

    Ok(Json(json!({ "ok": true, "files": written })))
}

fn unpack(
    bytes: &[u8],
    destination: &Path,
    root: &Path,
    binary: &Path,
) -> Result<i64, String> {
    let cursor = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| e.to_string())?;
    let mut written = 0i64;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;

        // `enclosed_name` refuses absolute paths and anything with `..`.
        let Some(safe) = entry.enclosed_name() else {
            return Err(format!("unsafe path in archive: {}", entry.name()));
        };
        let target = destination.join(&safe);

        if !target.starts_with(root) {
            return Err(format!("entry escapes the server folder: {}", entry.name()));
        }
        // Never let an archive replace the program the panel launches.
        if target == binary {
            return Err("archive tries to replace the server program".to_string());
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
        written += 1;
    }

    Ok(written)
}

#[derive(Debug, Deserialize)]
pub struct CopyRequest {
    pub from: String,
    pub to: String,
}

/// Copies a file or a whole folder to another place under the server root.
pub async fn copy(
    State(state): State<AppState>,
    user: CurrentUser,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<CopyRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let root = root_for(&state, &user, &id).await?;
    let from = resolve(&root, &body.from)?;
    let to = resolve(&root, &body.to)?;
    deny_if_protected(&state, &id, &to).await?;

    if to.exists() {
        return Err(AppError::Conflict("something is already there".into()));
    }

    let source = from.clone();
    let target = to.clone();
    let copied = tokio::task::spawn_blocking(move || copy_tree(&source, &target))
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!("copy task failed: {e}")))?
        .map_err(AppError::BadRequest)?;

    audit(
        &state,
        Some(&user),
        Some(&id),
        "files.copy",
        Some(&format!("{} -> {}", body.from, body.to)),
    )
    .await;

    Ok(Json(json!({ "ok": true, "files": copied })))
}

fn copy_tree(from: &Path, to: &Path) -> Result<i64, String> {
    if from.is_file() {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::copy(from, to).map_err(|e| e.to_string())?;
        return Ok(1);
    }

    let mut copied = 0i64;
    for entry in walkdir::WalkDir::new(from).follow_links(false) {
        let entry = entry.map_err(|e| e.to_string())?;
        let relative = entry.path().strip_prefix(from).map_err(|e| e.to_string())?;
        let target = to.join(relative);

        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::copy(entry.path(), &target).map_err(|e| e.to_string())?;
            copied += 1;
        }
    }
    Ok(copied)
}
