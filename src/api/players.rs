// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
//! Player administration.
//!
//! Live players come from the server's query port; operators, bans and the
//! whitelist are read straight from the JSON files Pumpkin keeps in `data/`.
//! Every mutating action is performed by sending the matching console command,
//! so the server stays the single source of truth and the files update
//! themselves.

use crate::api::load_server;
use crate::auth::{audit, server_access, CurrentUser};
use crate::error::{AppError, AppResult};
use crate::query::{self, QueryStatus};
use crate::AppState;
use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
pub struct PlayerEntry {
    pub name: String,
    pub uuid: Option<String>,
    /// Present on ban entries.
    pub reason: Option<String>,
    pub source: Option<String>,
    pub expires: Option<String>,
    /// Present on operator entries.
    pub level: Option<i64>,
}

fn string_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key).and_then(|v| v.as_str()).map(str::to_owned)
}

/// Reads one of Pumpkin's `data/*.json` files, tolerating a missing file.
async fn read_entries(dir: &std::path::Path, file: &str, name_key: &str) -> Vec<PlayerEntry> {
    let path = dir.join("data").join(file);
    let Ok(text) = tokio::fs::read_to_string(&path).await else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(items) = value.as_array() else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(|item| {
            Some(PlayerEntry {
                name: string_field(item, name_key)?,
                uuid: string_field(item, "uuid"),
                reason: string_field(item, "reason"),
                source: string_field(item, "source"),
                expires: string_field(item, "expires"),
                level: item.get("level").and_then(serde_json::Value::as_i64),
            })
        })
        .collect()
}

/// Works out where the query service is listening for a given server.
async fn query_address(working_dir: &std::path::Path) -> Option<SocketAddr> {
    let text = tokio::fs::read_to_string(working_dir.join("pumpkin.toml"))
        .await
        .ok()?;
    let doc = text.parse::<toml_edit::DocumentMut>().ok()?;

    let query_table = doc.get("networking")?.get("query")?;
    if !query_table
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return None;
    }

    let listen = query_table.get("address")?.as_str()?;
    // The server may bind 0.0.0.0; the panel always talks to it over loopback.
    let port = listen.rsplit(':').next()?.parse::<u16>().ok()?;
    Some(SocketAddr::from(([127, 0, 0, 1], port)))
}

#[derive(Debug, Serialize)]
pub struct PlayersResponse {
    pub online: Vec<String>,
    pub online_count: u32,
    pub max_players: u32,
    pub operators: Vec<PlayerEntry>,
    pub banned: Vec<PlayerEntry>,
    pub banned_ips: Vec<PlayerEntry>,
    pub whitelist: Vec<PlayerEntry>,
    pub known: Vec<PlayerEntry>,
    /// Where `online` came from: `"query"` for the server's query port, or
    /// `"log"` when it was reconstructed from the server's log because the
    /// query port is off. `None` when the server is not running.
    pub online_source: Option<&'static str>,
    /// Set when no live list could be produced at all, e.g. the server is
    /// stopped, or the query listener is off *and* there is no readable log.
    pub query_error: Option<String>,
}

/// Reads `max_players` from the Java section of `pumpkin.toml`, for when the
/// query port that would normally report it is unavailable.
async fn configured_max_players(working_dir: &std::path::Path) -> u32 {
    let Ok(text) = tokio::fs::read_to_string(working_dir.join("pumpkin.toml")).await else {
        return 0;
    };
    text.parse::<toml_edit::DocumentMut>()
        .ok()
        .and_then(|doc| {
            doc.get("networking")?
                .get("java")?
                .get("max_players")?
                .as_integer()
        })
        .map_or(0, |n| n.max(0) as u32)
}

pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<PlayersResponse>> {
    let access = server_access(&state, &user, &id).await?;
    if !access.console {
        return Err(AppError::Forbidden);
    }

    let server = load_server(&state, &id).await?;
    let dir = PathBuf::from(&server.working_dir);

    let running = matches!(
        state.sup.instance(&id).await.runtime().await.status,
        crate::supervisor::Status::Running
    );

    let (mut status, mut query_error, mut online_source): (
        QueryStatus,
        Option<String>,
        Option<&'static str>,
    ) = if running {
        match query_address(&dir).await {
            Some(addr) => match query::query(addr).await {
                Ok(status) => (status, None, Some("query")),
                Err(e) => (QueryStatus::default(), Some(e), None),
            },
            None => (
                QueryStatus::default(),
                Some("query is disabled in pumpkin.toml ([networking.query])".into()),
                None,
            ),
        }
    } else {
        (QueryStatus::default(), Some("server is not running".into()), None)
    };

    // Without the query port the panel would report an empty server forever.
    // The server's own log records every join and leave, so fall back to that.
    if running && online_source.is_none() {
        let log = dir.join("logs").join("latest.log");
        if let Some(players) = crate::logplayers::online(&id, &log).await {
            status.online = players.len() as u32;
            status.max = configured_max_players(&dir).await;
            status.players = players;
            online_source = Some("log");
            query_error = None;
        }
    }

    Ok(Json(PlayersResponse {
        online_source,
        online_count: status.online,
        max_players: status.max,
        online: status.players,
        operators: read_entries(&dir, "ops.json", "name").await,
        banned: read_entries(&dir, "banned-players.json", "name").await,
        banned_ips: read_entries(&dir, "banned-ips.json", "ip").await,
        whitelist: read_entries(&dir, "whitelist.json", "name").await,
        known: read_entries(&dir, "usercache.json", "name").await,
        query_error,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayerAction {
    Op,
    Deop,
    Kick,
    Ban,
    Pardon,
    BanIp,
    PardonIp,
    Heal,
    Feed,
    Gamemode,
    WhitelistAdd,
    WhitelistRemove,
    Kill,
}

#[derive(Debug, Deserialize)]
pub struct ActionRequest {
    pub action: PlayerAction,
    /// Player name, or an IP address for the IP ban actions.
    pub target: String,
    /// Ban reason, or the gamemode name.
    #[serde(default)]
    pub value: Option<String>,
}

/// Rejects anything that could smuggle a second command onto the console line.
fn clean(input: &str, what: &str) -> AppResult<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest(format!("{what} is required")));
    }
    if trimmed.len() > 64
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'))
    {
        return Err(AppError::BadRequest(format!("{what} is not a valid value")));
    }
    Ok(trimmed.to_string())
}

pub async fn act(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<ActionRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let access = server_access(&state, &user, &id).await?;
    // Player administration is a console capability, since that is how it runs.
    if !access.console {
        return Err(AppError::Forbidden);
    }

    let target = clean(&body.target, "player name")?;

    // Reasons are free text, so they are sanitised separately: newlines out,
    // length capped, and they are always the trailing argument.
    let reason = body
        .value
        .as_deref()
        .map(|r| {
            r.replace(['\n', '\r'], " ")
                .chars()
                .take(120)
                .collect::<String>()
        })
        .filter(|r| !r.trim().is_empty());

    let command = match body.action {
        PlayerAction::Op => format!("op {target}"),
        PlayerAction::Deop => format!("deop {target}"),
        PlayerAction::Kick => match &reason {
            Some(r) => format!("kick {target} {r}"),
            None => format!("kick {target}"),
        },
        PlayerAction::Ban => match &reason {
            Some(r) => format!("ban {target} {r}"),
            None => format!("ban {target}"),
        },
        PlayerAction::Pardon => format!("pardon {target}"),
        PlayerAction::BanIp => match &reason {
            Some(r) => format!("ban-ip {target} {r}"),
            None => format!("ban-ip {target}"),
        },
        PlayerAction::PardonIp => format!("pardon-ip {target}"),
        PlayerAction::Kill => format!("kill {target}"),
        PlayerAction::WhitelistAdd => format!("whitelist add {target}"),
        PlayerAction::WhitelistRemove => format!("whitelist remove {target}"),
        // Pumpkin has no /heal or /feed. Instant health at a high amplifier
        // refills hearts, and the saturation effect refills the food bar; both
        // are hidden so the player does not see particles.
        PlayerAction::Heal => {
            format!("effect give {target} minecraft:instant_health 1 100 true")
        }
        PlayerAction::Feed => {
            format!("effect give {target} minecraft:saturation 1 20 true")
        }
        PlayerAction::Gamemode => {
            let mode = clean(reason.as_deref().unwrap_or("survival"), "gamemode")?;
            if !matches!(
                mode.as_str(),
                "survival" | "creative" | "adventure" | "spectator"
            ) {
                return Err(AppError::BadRequest("unknown gamemode".into()));
            }
            format!("gamemode {mode} {target}")
        }
    };

    state
        .sup
        .instance(&id)
        .await
        .send_command(&command)
        .await?;

    audit(&state, Some(&user), Some(&id), "player.action", Some(&command)).await;
    Ok(Json(json!({ "ok": true, "command": command })))
}
