// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
//! Minecraft Query (GameSpy4) client.
//!
//! Pumpkin enables this on the Java port by default, and it is the cleanest way
//! to read live server state: it returns the full online player list without
//! sending anything through the console.

use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::UdpSocket;

const TIMEOUT: Duration = Duration::from_secs(3);
const SESSION_ID: i32 = 1;
const SPLIT_MARKER: &[u8] = b"splitnum\x00\x80\x00";
const PLAYER_MARKER: &[u8] = b"\x00\x01player_\x00\x00";

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct QueryStatus {
    pub motd: String,
    pub map: String,
    pub version: String,
    pub online: u32,
    pub max: u32,
    pub players: Vec<String>,
}

/// Finds the byte offset of `needle` inside `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

pub async fn query(addr: SocketAddr) -> Result<QueryStatus, String> {
    let socket = UdpSocket::bind(if addr.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" })
        .await
        .map_err(|e| format!("could not open a UDP socket: {e}"))?;

    socket
        .connect(addr)
        .await
        .map_err(|e| format!("could not reach {addr}: {e}"))?;

    // Handshake: the server answers with a challenge token as ASCII digits.
    let mut handshake = vec![0xFE, 0xFD, 0x09];
    handshake.extend_from_slice(&SESSION_ID.to_be_bytes());
    socket
        .send(&handshake)
        .await
        .map_err(|e| format!("query handshake failed: {e}"))?;

    let mut buffer = [0u8; 4096];
    let read = tokio::time::timeout(TIMEOUT, socket.recv(&mut buffer))
        .await
        .map_err(|_| "query timed out".to_string())?
        .map_err(|e| format!("query failed: {e}"))?;

    if read < 6 {
        return Err("query handshake response was too short".into());
    }

    let token: i32 = std::str::from_utf8(&buffer[5..read])
        .ok()
        .and_then(|s| s.trim_end_matches('\0').trim().parse().ok())
        .ok_or_else(|| "could not read the query challenge token".to_string())?;

    // Full stat request: session, challenge, then four bytes of padding.
    let mut request = vec![0xFE, 0xFD, 0x00];
    request.extend_from_slice(&SESSION_ID.to_be_bytes());
    request.extend_from_slice(&token.to_be_bytes());
    request.extend_from_slice(&[0, 0, 0, 0]);
    socket
        .send(&request)
        .await
        .map_err(|e| format!("query request failed: {e}"))?;

    let read = tokio::time::timeout(TIMEOUT, socket.recv(&mut buffer))
        .await
        .map_err(|_| "query timed out".to_string())?
        .map_err(|e| format!("query failed: {e}"))?;

    parse_full_stat(&buffer[..read])
}

fn parse_full_stat(packet: &[u8]) -> Result<QueryStatus, String> {
    if packet.len() < 6 {
        return Err("query response was too short".into());
    }

    let body = &packet[5..];
    let start = find(body, SPLIT_MARKER).ok_or_else(|| "malformed query response".to_string())?;
    let body = &body[start + SPLIT_MARKER.len()..];

    let split = find(body, PLAYER_MARKER).ok_or_else(|| "malformed query response".to_string())?;
    let (kv_bytes, player_bytes) = (&body[..split], &body[split + PLAYER_MARKER.len()..]);

    // Key/value pairs are null-terminated strings in alternating order.
    let fields: Vec<String> = kv_bytes
        .split(|b| *b == 0)
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect();

    let mut status = QueryStatus::default();
    for pair in fields.chunks(2) {
        let [key, value] = pair else { continue };
        match key.as_str() {
            "hostname" => status.motd = value.clone(),
            "map" => status.map = value.clone(),
            "version" => status.version = value.clone(),
            "numplayers" => status.online = value.parse().unwrap_or(0),
            "maxplayers" => status.max = value.parse().unwrap_or(0),
            _ => {}
        }
    }

    status.players = player_bytes
        .split(|b| *b == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect();

    Ok(status)
}
