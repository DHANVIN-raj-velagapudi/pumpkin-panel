// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
//! Pumpkin-specific knowledge.
//!
//! This is what separates the panel from a generic process manager: it knows
//! what Pumpkin logs on startup, how its plugin folder is laid out, what the
//! permission model in `pumpkin.toml` means, and where the plugin marketplace
//! lives.

use crate::api::load_server;
use crate::auth::{server_access, CurrentUser};
use crate::error::{AppError, AppResult};
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

const MARKETPLACE: &str = "https://market.pumpkinmc.org";

/// Everything Pumpkin announces about itself in its first log line.
#[derive(Debug, Default, Clone, Serialize)]
pub struct VersionInfo {
    pub pumpkin_version: Option<String>,
    pub java_version: Option<String>,
    pub java_protocol: Option<u32>,
    pub bedrock_version: Option<String>,
    pub bedrock_protocol: Option<u32>,
}

/// Parses the startup banner, which looks like:
///
/// ```text
/// Starting Pumpkin 0.1.0-dev+26.2-26.40 Java Minecraft (Protocol 776) | 1.26.40 Bedrock (Protocol 2168)
/// ```
///
/// Reading the line the server already prints avoids inventing a second source
/// of truth about which version is actually running.
pub fn parse_version_banner(line: &str) -> Option<VersionInfo> {
    let plain = strip_ansi(line);
    let start = plain.find("Starting Pumpkin ")?;
    let rest = &plain[start + "Starting Pumpkin ".len()..];

    let mut info = VersionInfo {
        pumpkin_version: rest.split_whitespace().next().map(str::to_owned),
        ..VersionInfo::default()
    };

    // The two halves are separated by a pipe: Java on the left, Bedrock right.
    let (java_part, bedrock_part) = match rest.split_once('|') {
        Some((a, b)) => (a, Some(b)),
        None => (rest, None),
    };

    // `0.1.0-dev+26.2-26.40` carries both game versions after the `+`.
    if let Some(build) = info
        .pumpkin_version
        .as_ref()
        .and_then(|v| v.split_once('+').map(|(_, b)| b.to_string()))
    {
        let mut halves = build.split('-');
        info.java_version = halves.next().map(str::to_owned);
        info.bedrock_version = halves.next().map(str::to_owned);
    }

    info.java_protocol = extract_protocol(java_part);
    if let Some(bedrock) = bedrock_part {
        info.bedrock_protocol = extract_protocol(bedrock);
    }

    Some(info)
}

fn extract_protocol(text: &str) -> Option<u32> {
    let at = text.find("Protocol ")?;
    text[at + "Protocol ".len()..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Skip until the terminating letter of the escape sequence.
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[derive(Debug, Serialize)]
pub struct PluginFile {
    pub file: String,
    pub size_bytes: u64,
    pub modified: i64,
    /// Present when the file carries marketplace metadata.
    pub name: Option<String>,
    pub version: Option<String>,
    pub developer: Option<String>,
    pub paid: Option<bool>,
    /// True when the file is a WebAssembly component.
    pub is_wasm: bool,
}

/// Looks for the marketplace metadata a published plugin carries.
///
/// The panel has no WebAssembly runtime, so a plugin's own `PluginMetadata` —
/// which Pumpkin obtains by instantiating the component and calling into it —
/// is out of reach. Marketplace builds additionally embed a small JSON blob,
/// and that *is* readable by scanning the bytes. Plugins built locally simply
/// have no metadata to find, which is why every field here is optional.
fn scan_marketplace_metadata(bytes: &[u8]) -> Option<serde_json::Value> {
    let needle = b"marketplace_url";
    let position = bytes
        .windows(needle.len())
        .position(|window| window == needle)?;

    // Walk back to the opening brace of the object containing the key.
    let start = bytes[..position].iter().rposition(|b| *b == b'{')?;

    // Then forward to the matching close, tracking nesting.
    let mut depth = 0i32;
    let mut end = None;
    for (offset, byte) in bytes[start..].iter().enumerate().take(4096) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(start + offset + 1);
                    break;
                }
            }
            _ => {}
        }
    }

    let slice = &bytes[start..end?];
    serde_json::from_slice(slice).ok()
}

async fn read_plugins(dir: &std::path::Path) -> Vec<PluginFile> {
    let mut plugins = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return plugins;
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let Ok(meta) = entry.metadata().await else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }

        let file = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();

        // WebAssembly binaries start with a four byte magic number.
        let head = tokio::fs::read(&path).await.unwrap_or_default();
        let is_wasm = head.starts_with(b"\0asm");
        let metadata = scan_marketplace_metadata(&head);

        plugins.push(PluginFile {
            file,
            size_bytes: meta.len(),
            modified: meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64),
            name: metadata
                .as_ref()
                .and_then(|m| m.get("plugin_name"))
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            version: metadata
                .as_ref()
                .and_then(|m| m.get("version"))
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            developer: metadata
                .as_ref()
                .and_then(|m| m.get("dev_name"))
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            paid: metadata
                .as_ref()
                .and_then(|m| m.get("is_paid"))
                .and_then(serde_json::Value::as_bool),
            is_wasm,
        });
    }

    plugins.sort_by(|a, b| a.file.to_lowercase().cmp(&b.file.to_lowercase()));
    plugins
}

/// The plugin sandbox policy, read out of `pumpkin.toml`.
#[derive(Debug, Default, Serialize)]
pub struct PluginPolicy {
    pub enabled: bool,
    pub hot_reload: bool,
    pub allow_unsigned: bool,
    pub ask_permission_confirmation: bool,
    pub allowed_permissions: Vec<String>,
    pub blocked_permissions: Vec<String>,
    pub inherit_env: bool,
    pub loopback_only: bool,
}

async fn read_policy(working_dir: &std::path::Path) -> PluginPolicy {
    let Ok(text) = tokio::fs::read_to_string(working_dir.join("pumpkin.toml")).await else {
        return PluginPolicy::default();
    };
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else {
        return PluginPolicy::default();
    };
    let Some(table) = doc.get("plugins") else {
        return PluginPolicy::default();
    };

    let flag = |key: &str| table.get(key).and_then(|v| v.as_bool()).unwrap_or(false);
    let list = |key: &str| {
        table
            .get(key)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };

    PluginPolicy {
        enabled: flag("enabled"),
        hot_reload: flag("hot_reload"),
        allow_unsigned: flag("allow_unsigned"),
        ask_permission_confirmation: flag("ask_permission_confirmation"),
        allowed_permissions: list("allowed_permissions"),
        blocked_permissions: list("blocked_permissions"),
        inherit_env: flag("inherit_env"),
        loopback_only: flag("loopback_only"),
    }
}

pub async fn overview(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    server_access(&state, &user, &id).await?;
    let server = load_server(&state, &id).await?;
    let working_dir = PathBuf::from(&server.working_dir);

    // The banner is only in the console once the server has started at least
    // once this session.
    let version = state
        .sup
        .instance(&id)
        .await
        .history()
        .await
        .iter()
        .find_map(|line| parse_version_banner(&line.line));

    Ok(Json(json!({
        "version": version,
        "plugins": read_plugins(&working_dir.join("plugins")).await,
        "policy": read_policy(&working_dir).await,
    })))
}

#[derive(Debug, Deserialize)]
pub struct MarketQuery {
    #[serde(default)]
    pub search: Option<String>,
}

/// Proxies the public plugin marketplace.
///
/// Going through the backend rather than calling it from the browser keeps the
/// page's content security policy locked to `self`, and means the marketplace
/// never sees the panel user's address.
pub async fn marketplace(
    State(_state): State<AppState>,
    _user: CurrentUser,
    Query(query): Query<MarketQuery>,
) -> AppResult<Json<serde_json::Value>> {
    let response = reqwest_get(&format!("{MARKETPLACE}/api/plugins")).await?;

    let mut plugins: Vec<serde_json::Value> = serde_json::from_str(&response)
        .map_err(|e| AppError::Other(anyhow::anyhow!("marketplace returned unusable data: {e}")))?;

    if let Some(search) = query.search.as_deref().map(str::to_lowercase) {
        if !search.trim().is_empty() {
            plugins.retain(|p| {
                let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let category = p.get("category").and_then(|v| v.as_str()).unwrap_or("");
                let dev = p.get("dev_name").and_then(|v| v.as_str()).unwrap_or("");
                name.to_lowercase().contains(&search)
                    || category.to_lowercase().contains(&search)
                    || dev.to_lowercase().contains(&search)
            });
        }
    }

    Ok(Json(json!({ "plugins": plugins, "source": MARKETPLACE })))
}

/// Minimal HTTP GET, so the panel does not take on a full HTTP client just to
/// read one public JSON endpoint.
async fn reqwest_get(url: &str) -> AppResult<String> {
    let url = url.to_string();
    tokio::task::spawn_blocking(move || {
        ureq::get(&url)
            .timeout(std::time::Duration::from_secs(10))
            .call()
            .map_err(|e| format!("could not reach the marketplace: {e}"))?
            .into_string()
            .map_err(|e| format!("marketplace response was unreadable: {e}"))
    })
    .await
    .map_err(|e| AppError::Other(anyhow::anyhow!("marketplace lookup failed: {e}")))?
    .map_err(AppError::BadRequest)
}
