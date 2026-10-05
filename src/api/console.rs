// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
use crate::api::load_server;
use crate::auth::{audit, server_access, CurrentUser};
use crate::error::{AppError, AppResult};
use crate::supervisor::ConsoleLine;
use crate::AppState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::Response;
use axum::Json;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

pub async fn history(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let access = server_access(&state, &user, &id).await?;
    if !access.console {
        return Err(AppError::Forbidden);
    }

    let instance = state.sup.instance(&id).await;
    Ok(Json(json!({
        "lines": instance.history().await,
        "runtime": instance.runtime().await,
    })))
}

#[derive(Debug, Deserialize)]
pub struct CommandRequest {
    pub command: String,
}

pub async fn command(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    Json(body): Json<CommandRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let access = server_access(&state, &user, &id).await?;
    if !access.console {
        return Err(AppError::Forbidden);
    }

    let command = body.command.trim();
    if command.is_empty() {
        return Err(AppError::BadRequest("command is empty".into()));
    }

    state.sup.instance(&id).await.send_command(command).await?;
    audit(&state, Some(&user), Some(&id), "console.command", Some(command)).await;

    Ok(Json(json!({ "ok": true })))
}

pub async fn websocket(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> AppResult<Response> {
    let access = server_access(&state, &user, &id).await?;
    if !access.console {
        return Err(AppError::Forbidden);
    }
    // Fail early if the server was deleted between page load and connect.
    load_server(&state, &id).await?;

    Ok(ws.on_upgrade(move |socket| stream_console(socket, state, user, id)))
}

fn encode(line: &ConsoleLine) -> String {
    serde_json::to_string(line).unwrap_or_else(|_| "{}".to_string())
}

async fn stream_console(socket: WebSocket, state: AppState, user: CurrentUser, id: String) {
    let instance = state.sup.instance(&id).await;
    // Subscribe before replaying history so nothing is missed in between.
    let mut rx = instance.subscribe();
    // Split so the outgoing half can be written to while the incoming half is
    // still being polled for commands.
    let (mut sender, mut receiver) = socket.split();

    for line in instance.history().await {
        if sender.send(Message::Text(encode(&line).into())).await.is_err() {
            return;
        }
    }

    loop {
        tokio::select! {
            received = rx.recv() => match received {
                Ok(line) => {
                    if sender.send(Message::Text(encode(&line).into())).await.is_err() {
                        return;
                    }
                }
                Err(RecvError::Lagged(skipped)) => {
                    let notice = json!({
                        "stream": "system",
                        "line": format!("[panel] console fell behind, {skipped} lines skipped"),
                        "at": crate::db::now(),
                    });
                    if sender.send(Message::Text(notice.to_string().into())).await.is_err() {
                        return;
                    }
                }
                Err(RecvError::Closed) => return,
            },
            incoming = receiver.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    let command = text.as_str().trim().to_string();
                    if command.is_empty() {
                        continue;
                    }
                    if let Err(e) = instance.send_command(&command).await {
                        let notice = json!({
                            "stream": "system",
                            "line": format!("[panel] {e}"),
                            "at": crate::db::now(),
                        });
                        if sender.send(Message::Text(notice.to_string().into())).await.is_err() {
                            return;
                        }
                        continue;
                    }
                    audit(
                        &state,
                        Some(&user),
                        Some(&id),
                        "console.command",
                        Some(&command),
                    )
                    .await;
                }
                Some(Ok(Message::Close(_))) | None => return,
                Some(Err(_)) => return,
                Some(Ok(_)) => {}
            },
        }
    }
}
