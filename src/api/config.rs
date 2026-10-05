// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
//! Schema-aware editing of a server's TOML configuration.
//!
//! A generic panel hands you a text box and wishes you luck. Pumpkin's
//! `pumpkin.toml` has around a hundred keys across thirty tables, so this module
//! flattens the document into typed fields the UI can render as real form
//! controls, and writes changes back with `toml_edit` so comments, ordering and
//! formatting all survive the round trip.

use crate::api::load_server;
use crate::auth::{audit, server_access, CurrentUser};
use crate::error::{AppError, AppResult};
use crate::AppState;
use axum::extract::{Path as AxumPath, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use toml_edit::{DocumentMut, Item, Value};

/// Config files are never larger than this in practice.
const MAX_CONFIG_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Serialize)]
pub struct Field {
    /// Dotted path, e.g. `networking.java.motd`.
    pub key: String,
    /// Table this field belongs to, e.g. `networking.java`. Empty for top level.
    pub section: String,
    /// Final path segment, used as the form label.
    pub name: String,
    pub value: serde_json::Value,
    /// One of: string, integer, float, boolean, array, unsupported.
    pub kind: &'static str,
    /// Comment that sits above the key in the file, if any.
    pub description: Option<String>,
}

/// Same path safety rules as the file manager: no absolute paths, no traversal.
fn resolve_config(root: &Path, relative: &str) -> AppResult<PathBuf> {
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

    if safe.extension().and_then(|e| e.to_str()) != Some("toml") {
        return Err(AppError::BadRequest("only .toml files can be edited here".into()));
    }

    match safe.canonicalize() {
        Ok(resolved) if resolved.starts_with(&root) => Ok(safe),
        Ok(_) => Err(AppError::BadRequest("path escapes the server directory".into())),
        Err(_) => Err(AppError::NotFound("config file".into())),
    }
}

async fn root_for(
    state: &AppState,
    user: &CurrentUser,
    id: &str,
) -> AppResult<PathBuf> {
    let access = server_access(state, user, id).await?;
    if !access.config {
        return Err(AppError::Forbidden);
    }
    let server = load_server(state, id).await?;
    Ok(PathBuf::from(server.working_dir))
}

fn toml_value_to_json(value: &Value) -> serde_json::Value {
    match value {
        Value::String(s) => json!(s.value()),
        Value::Integer(i) => json!(i.value()),
        Value::Float(f) => json!(f.value()),
        Value::Boolean(b) => json!(b.value()),
        Value::Datetime(d) => json!(d.value().to_string()),
        Value::Array(a) => {
            serde_json::Value::Array(a.iter().map(toml_value_to_json).collect())
        }
        Value::InlineTable(t) => {
            let mut map = serde_json::Map::new();
            for (k, v) in t.iter() {
                map.insert(k.to_string(), toml_value_to_json(v));
            }
            serde_json::Value::Object(map)
        }
    }
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::String(_) => "string",
        Value::Integer(_) => "integer",
        Value::Float(_) => "float",
        Value::Boolean(_) => "boolean",
        Value::Array(_) => "array",
        Value::Datetime(_) | Value::InlineTable(_) => "unsupported",
    }
}

/// Collects the comment block sitting above each key, keyed by dotted path.
///
/// This reads the raw text rather than `toml_edit` decor, because the decor
/// accessors have moved between versions and a line scan is stable across all
/// of them. Steel ships heavily commented configs, so this turns its own
/// documentation into inline help for free.
fn comments_from_raw(raw: &str) -> BTreeMap<String, String> {
    let mut comments = BTreeMap::new();
    let mut section = String::new();
    let mut pending: Vec<String> = Vec::new();

    for line in raw.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            pending.clear();
            continue;
        }

        // Skip editor directives such as `#:schema https://...`.
        if let Some(comment) = trimmed.strip_prefix('#') {
            if !comment.starts_with(':') {
                pending.push(comment.trim().to_string());
            }
            continue;
        }

        if trimmed.starts_with('[') {
            section = trimmed
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim()
                .to_string();
            pending.clear();
            continue;
        }

        if let Some((key, _)) = trimmed.split_once('=') {
            let key = key.trim().trim_matches('"');
            if !pending.is_empty() {
                let path = if section.is_empty() {
                    key.to_string()
                } else {
                    format!("{section}.{key}")
                };
                comments.insert(path, pending.join(" "));
            }
        }
        pending.clear();
    }

    comments
}

fn walk(item: &Item, prefix: &str, out: &mut Vec<Field>, comments: &BTreeMap<String, String>) {
    let Some(table) = item.as_table_like() else {
        return;
    };

    for (key, value) in table.iter() {
        let path = if prefix.is_empty() {
            key.to_string()
        } else {
            format!("{prefix}.{key}")
        };

        match value {
            Item::Value(v) => out.push(Field {
                section: prefix.to_string(),
                name: key.to_string(),
                value: toml_value_to_json(v),
                kind: kind_of(v),
                description: comments.get(&path).cloned(),
                key: path,
            }),
            Item::Table(_) => walk(value, &path, out, comments),
            // Arrays of tables (Steel's `[[domains.*.worlds]]`) stay raw-only.
            Item::ArrayOfTables(_) | Item::None => {}
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ConfigQuery {
    #[serde(default = "default_config_file")]
    pub file: String,
}

fn default_config_file() -> String {
    "pumpkin.toml".to_string()
}

pub async fn read(
    State(state): State<AppState>,
    user: CurrentUser,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<ConfigQuery>,
) -> AppResult<Json<serde_json::Value>> {
    let root = root_for(&state, &user, &id).await?;
    let path = resolve_config(&root, &query.file)?;

    let meta = tokio::fs::metadata(&path).await?;
    if meta.len() > MAX_CONFIG_BYTES {
        return Err(AppError::BadRequest("config file is unexpectedly large".into()));
    }

    let raw = tokio::fs::read_to_string(&path).await?;
    let doc = raw
        .parse::<DocumentMut>()
        .map_err(|e| AppError::BadRequest(format!("{} is not valid TOML: {e}", query.file)))?;

    let comments = comments_from_raw(&raw);
    let mut fields = Vec::new();
    walk(doc.as_item(), "", &mut fields, &comments);

    // Section names are owned rather than borrowed from `fields`, which is
    // moved into the response below.
    let section_names: Vec<String> = fields
        .iter()
        .map(|field| field.section.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();

    Ok(Json(json!({
        "file": query.file,
        "raw": raw,
        "fields": fields,
        "sections": section_names,
    })))
}

#[derive(Debug, Deserialize)]
pub struct WriteConfig {
    #[serde(default = "default_config_file")]
    pub file: String,
    /// Dotted key -> new value. Applied in place, preserving comments.
    #[serde(default)]
    pub updates: BTreeMap<String, serde_json::Value>,
    /// Full replacement of the file. Mutually exclusive with `updates`.
    #[serde(default)]
    pub raw: Option<String>,
}

fn json_to_toml_value(value: &serde_json::Value) -> AppResult<Value> {
    Ok(match value {
        serde_json::Value::Bool(b) => Value::from(*b),
        serde_json::Value::String(s) => Value::from(s.as_str()),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::from(i)
            } else if let Some(f) = n.as_f64() {
                Value::from(f)
            } else {
                return Err(AppError::BadRequest(format!("unsupported number: {n}")));
            }
        }
        serde_json::Value::Array(items) => {
            let mut array = toml_edit::Array::new();
            for item in items {
                array.push(json_to_toml_value(item)?);
            }
            Value::Array(array)
        }
        serde_json::Value::Null | serde_json::Value::Object(_) => {
            return Err(AppError::BadRequest(
                "only scalars and arrays can be set through the form editor".into(),
            ))
        }
    })
}

/// Sets a dotted key, keeping the surrounding whitespace and comments intact.
fn set_dotted(doc: &mut DocumentMut, key: &str, value: &serde_json::Value) -> AppResult<()> {
    let parts: Vec<&str> = key.split('.').filter(|p| !p.is_empty()).collect();
    let Some((last, parents)) = parts.split_last() else {
        return Err(AppError::BadRequest("empty config key".into()));
    };

    let mut item: &mut Item = doc.as_item_mut();
    for parent in parents {
        item = item
            .as_table_like_mut()
            .and_then(|t| t.get_mut(parent))
            .ok_or_else(|| AppError::BadRequest(format!("no such config section: {parent}")))?;
    }

    let table = item
        .as_table_like_mut()
        .ok_or_else(|| AppError::BadRequest(format!("{key} is not inside a table")))?;

    let new_value = json_to_toml_value(value)?;

    match table.get_mut(last).and_then(Item::as_value_mut) {
        Some(existing) => {
            // Carry the original decor across so spacing and trailing comments stay.
            let decor = existing.decor().clone();
            let mut replacement = new_value;
            *replacement.decor_mut() = decor;
            *existing = replacement;
        }
        None => {
            table.insert(last, Item::Value(new_value));
        }
    }

    Ok(())
}

pub async fn write(
    State(state): State<AppState>,
    user: CurrentUser,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<WriteConfig>,
) -> AppResult<Json<serde_json::Value>> {
    let root = root_for(&state, &user, &id).await?;
    let path = resolve_config(&root, &body.file)?;

    let updated = if let Some(raw) = body.raw {
        // Validate before touching disk, so a typo cannot leave a broken config.
        raw.parse::<DocumentMut>()
            .map_err(|e| AppError::BadRequest(format!("invalid TOML: {e}")))?;
        raw
    } else {
        if body.updates.is_empty() {
            return Err(AppError::BadRequest("no changes were supplied".into()));
        }
        let current = tokio::fs::read_to_string(&path).await?;
        let mut doc = current
            .parse::<DocumentMut>()
            .map_err(|e| AppError::BadRequest(format!("{} is not valid TOML: {e}", body.file)))?;

        for (key, value) in &body.updates {
            set_dotted(&mut doc, key, value)?;
        }
        doc.to_string()
    };

    // Keep a single-step backup, then swap the new file in atomically.
    let backup = path.with_extension("toml.bak");
    let _ = tokio::fs::copy(&path, &backup).await;

    let temp = path.with_extension("toml.panel-tmp");
    tokio::fs::write(&temp, updated.as_bytes()).await?;
    tokio::fs::rename(&temp, &path).await?;

    audit(
        &state,
        Some(&user),
        Some(&id),
        "config.write",
        Some(&body.file),
    )
    .await;

    Ok(Json(json!({
        "ok": true,
        "note": "restart the server for changes to take effect",
    })))
}

/// Lists the TOML files the panel knows how to edit, one level deep.
pub async fn list_configs(
    State(state): State<AppState>,
    user: CurrentUser,
    AxumPath(id): AxumPath<String>,
) -> AppResult<Json<serde_json::Value>> {
    let root = root_for(&state, &user, &id).await?;
    let mut files = Vec::new();

    let mut top = tokio::fs::read_dir(&root).await?;
    while let Some(entry) = top.next_entry().await? {
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.metadata().await.map(|m| m.is_dir()).unwrap_or(false);

        if !is_dir && name.ends_with(".toml") {
            files.push(name);
        } else if is_dir && name == "config" {
            // Steel keeps config.toml, groups.toml and worlds.toml in here.
            let mut nested = tokio::fs::read_dir(entry.path()).await?;
            while let Some(child) = nested.next_entry().await? {
                let child_name = child.file_name().to_string_lossy().to_string();
                if child_name.ends_with(".toml") {
                    files.push(format!("config/{child_name}"));
                }
            }
        }
    }

    files.sort();
    Ok(Json(json!({ "files": files })))
}
