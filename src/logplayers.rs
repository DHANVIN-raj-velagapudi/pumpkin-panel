// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
//! Who is online, worked out from the server's own log.
//!
//! The query port is the accurate source, but it is off in a stock-ish
//! `pumpkin.toml` of some setups and the panel has no other channel into a
//! server it did not start. Pumpkin does log every join and leave
//! (`NAME joined the game` / `NAME left the game`), so replaying those lines
//! recovers the same list without touching the server or its config.
//!
//! The tracker is incremental: it remembers how far into the file it has read
//! and only processes new lines, so polling every few seconds stays cheap even
//! for a log that has grown large.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

const JOINED: &str = " joined the game";
const LEFT: &str = " left the game";
/// The first line of every run, which is where a fresh player list begins.
const BANNER: &str = "Starting Pumpkin ";

#[derive(Debug, Default)]
struct Tracker {
    /// Bytes of the log already consumed, always ending on a line boundary.
    offset: u64,
    online: BTreeSet<String>,
}

static TRACKERS: OnceLock<Mutex<HashMap<String, Tracker>>> = OnceLock::new();

fn trackers() -> &'static Mutex<HashMap<String, Tracker>> {
    TRACKERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
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

/// A name as Minecraft allows it: letters, digits, `_`, plus the `.`, `-` and
/// space that Bedrock gamertags can carry. Anything else, such as the
/// `<Name> text` shape of a chat line, is not a join or leave.
fn is_player_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | ' '))
}

/// Pulls the player name out of a `... NAME joined the game` style line.
fn name_before(line: &str, suffix: &str) -> Option<String> {
    let body = line.trim_end().strip_suffix(suffix)?;
    let tokens: Vec<&str> = body.split_whitespace().collect();

    // Everything after the first log level is the message. The level may be
    // bracketed (`[INFO]`) or follow a timestamp (`18:37:55  INFO`). Using the
    // *first* one keeps a player who is literally named "INFO" working.
    let level = tokens.iter().position(|token| {
        matches!(
            token.trim_matches(|c| c == '[' || c == ']'),
            "INFO" | "WARN" | "ERROR" | "DEBUG" | "TRACE"
        )
    })?;

    let name = tokens[level + 1..].join(" ");
    is_player_name(&name).then_some(name)
}

impl Tracker {
    fn apply(&mut self, line: &str) {
        let line = strip_ansi(line);

        if line.contains(BANNER) {
            // A new run of the server: nobody carried over.
            self.online.clear();
        } else if let Some(name) = name_before(&line, JOINED) {
            self.online.insert(name);
        } else if let Some(name) = name_before(&line, LEFT) {
            self.online.remove(&name);
        }
    }

    /// Consumes `chunk`, which starts at `self.offset`, up to its last newline.
    fn feed(&mut self, chunk: &str) {
        let complete = chunk.rfind('\n').map_or(0, |i| i + 1);
        for line in chunk[..complete].lines() {
            self.apply(line);
        }
        self.offset += complete as u64;
    }
}

/// Reads any new log lines for `server_id` and returns who is online, sorted.
pub async fn online(server_id: &str, log: &Path) -> Option<Vec<String>> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    let mut file = tokio::fs::File::open(log).await.ok()?;
    let size = file.metadata().await.ok()?.len();

    let offset = {
        let mut map = trackers().lock().ok()?;
        let tracker = map.entry(server_id.to_string()).or_default();
        // A shrinking file means the log was rotated on a restart.
        if size < tracker.offset {
            *tracker = Tracker::default();
        }
        tracker.offset
    };

    if size > offset {
        file.seek(std::io::SeekFrom::Start(offset)).await.ok()?;
        let mut bytes = Vec::new();
        file.take(size - offset).read_to_end(&mut bytes).await.ok()?;
        let chunk = String::from_utf8_lossy(&bytes);

        let mut map = trackers().lock().ok()?;
        let tracker = map.get_mut(server_id)?;
        // Another poll may have advanced it while we were reading.
        if tracker.offset == offset {
            tracker.feed(&chunk);
        }
    }

    let map = trackers().lock().ok()?;
    Some(map.get(server_id)?.online.iter().cloned().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(lines: &[&str]) -> Vec<String> {
        let mut tracker = Tracker::default();
        let text = lines.join("\n") + "\n";
        tracker.feed(&text);
        tracker.online.into_iter().collect()
    }

    #[test]
    fn joins_and_leaves_are_tracked() {
        let online = run(&[
            "[INFO] Starting Pumpkin 0.2.0+26.3-26.51 Java Minecraft (Protocol 777)",
            "[INFO] Alice joined the game",
            "[INFO] Bob joined the game",
            "[INFO] Alice left the game",
            "[INFO] Alice joined the game",
            "[INFO] Bob left the game",
        ]);
        assert_eq!(online, ["Alice"]);
    }

    #[test]
    fn a_new_run_forgets_the_previous_one() {
        let online = run(&[
            "[INFO] Alice joined the game",
            "[INFO] Starting Pumpkin 0.2.0 Java Minecraft (Protocol 777)",
            "[INFO] Bob joined the game",
        ]);
        assert_eq!(online, ["Bob"]);
    }

    #[test]
    fn colour_codes_and_timestamps_are_tolerated() {
        let online = run(&[
            "\u{1b}[32m18:37:55  INFO\u{1b}[0m Carol joined the game",
            "18:38:02  INFO Dave joined the game",
        ]);
        assert_eq!(online, ["Carol", "Dave"]);
    }

    #[test]
    fn chat_that_imitates_a_join_is_ignored() {
        let online = run(&[
            "[INFO] <Mallory> Eve joined the game",
            "[INFO] [Chat] Mallory: Eve joined the game",
        ]);
        assert!(online.is_empty(), "{online:?}");
    }

    #[test]
    fn a_half_written_line_waits_for_the_rest() {
        let mut tracker = Tracker::default();
        tracker.feed("[INFO] Alice joined the game\n[INFO] Bob joi");
        assert_eq!(tracker.online.len(), 1);
        let consumed = tracker.offset;
        assert_eq!(consumed, "[INFO] Alice joined the game\n".len() as u64);
        // The caller re-reads from `offset`, so the next chunk starts mid-line.
        tracker.feed("[INFO] Bob joined the game\n");
        assert_eq!(tracker.online.len(), 2);
    }
}
