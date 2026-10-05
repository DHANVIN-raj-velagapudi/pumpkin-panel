// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
//! Security controls: request throttling, login lockout, and upload inspection.
//!
//! The guiding rule here is that a control has to stop a real attack without
//! getting in the way of ordinary use. Limits are therefore set well above what
//! a legitimate server owner does — uploading a 300 MB world archive is normal;
//! uploading something that claims to expand to 500 GB is not.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Rate limiting
// ---------------------------------------------------------------------------

struct Bucket {
    tokens: f64,
    last: Instant,
}

/// A token bucket per key, used to bound how fast one client can call the API.
pub struct RateLimiter {
    buckets: Mutex<HashMap<String, Bucket>>,
    capacity: f64,
    refill_per_sec: f64,
}

impl RateLimiter {
    pub fn new(capacity: f64, refill_per_sec: f64) -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
            capacity,
            refill_per_sec,
        }
    }

    /// Takes one token. Returns false when the caller is going too fast.
    pub fn allow(&self, key: &str) -> bool {
        let Ok(mut buckets) = self.buckets.lock() else {
            // A poisoned lock should never fail closed on a control-plane app.
            return true;
        };

        let now = Instant::now();

        // Opportunistic cleanup so idle clients do not accumulate forever.
        if buckets.len() > 4096 {
            buckets.retain(|_, b| now.duration_since(b.last) < Duration::from_secs(600));
        }

        let bucket = buckets.entry(key.to_string()).or_insert(Bucket {
            tokens: self.capacity,
            last: now,
        });

        let elapsed = now.duration_since(bucket.last).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        bucket.last = now;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Login lockout
// ---------------------------------------------------------------------------

struct Attempt {
    failures: u32,
    last_failure: Instant,
    locked_until: Option<Instant>,
}

/// Escalating lockout for repeated failed sign-ins.
///
/// Keyed on address *and* username so that one attacker cannot lock a
/// legitimate user out of their own account from elsewhere.
#[derive(Default)]
pub struct LoginThrottle {
    entries: Mutex<HashMap<String, Attempt>>,
}

/// Failures are forgiven after this long without a new one.
const FAILURE_WINDOW: Duration = Duration::from_secs(30 * 60);

impl LoginThrottle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the seconds remaining if this key is currently locked out.
    pub fn locked_for(&self, key: &str) -> Option<u64> {
        let mut entries = self.entries.lock().ok()?;
        let attempt = entries.get(key)?;
        let until = attempt.locked_until?;
        let now = Instant::now();

        if until > now {
            Some(until.duration_since(now).as_secs().max(1))
        } else {
            // Lockout has elapsed; let the next attempt through.
            entries.remove(key);
            None
        }
    }

    pub fn record_failure(&self, key: &str) {
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        let now = Instant::now();

        if entries.len() > 4096 {
            entries.retain(|_, a| now.duration_since(a.last_failure) < FAILURE_WINDOW);
        }

        let attempt = entries.entry(key.to_string()).or_insert(Attempt {
            failures: 0,
            last_failure: now,
            locked_until: None,
        });

        if now.duration_since(attempt.last_failure) > FAILURE_WINDOW {
            attempt.failures = 0;
        }

        attempt.failures += 1;
        attempt.last_failure = now;

        // Generous at first so a mistyped password is not punished, then steep.
        let penalty = match attempt.failures {
            0..=4 => None,
            5..=7 => Some(Duration::from_secs(30)),
            8..=11 => Some(Duration::from_secs(5 * 60)),
            _ => Some(Duration::from_secs(30 * 60)),
        };
        attempt.locked_until = penalty.map(|d| now + d);
    }

    pub fn record_success(&self, key: &str) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(key);
        }
    }
}

// ---------------------------------------------------------------------------
// Upload inspection
// ---------------------------------------------------------------------------

/// Largest single file the panel will accept.
pub const MAX_UPLOAD_BYTES: u64 = 512 * 1024 * 1024;

/// Archive limits. Chosen so a real world backup passes and a bomb does not.
const MAX_ARCHIVE_ENTRIES: usize = 20_000;
const MAX_UNCOMPRESSED_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_COMPRESSION_RATIO: f64 = 200.0;
/// Ratio is only meaningful once an archive is big enough to matter; a 1 KB
/// file of repeated zeroes has an enormous ratio and is entirely harmless.
const RATIO_CHECK_FLOOR: u64 = 32 * 1024 * 1024;

#[derive(Debug)]
pub struct ArchiveReport {
    pub entries: usize,
    pub compressed: u64,
    pub uncompressed: u64,
    pub ratio: f64,
    pub nested_archives: usize,
}

/// Reads a zip's central directory without extracting anything.
///
/// This is the check that catches a decompression bomb: the declared sizes are
/// read from the archive index, so nothing is ever written to disk in order to
/// find out how big it would be.
pub fn inspect_zip(bytes: &[u8]) -> Result<ArchiveReport, String> {
    let cursor = std::io::Cursor::new(bytes);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| format!("not a readable archive: {e}"))?;

    let entries = archive.len();
    if entries > MAX_ARCHIVE_ENTRIES {
        return Err(format!(
            "archive declares {entries} entries, more than the {MAX_ARCHIVE_ENTRIES} allowed"
        ));
    }

    let mut uncompressed: u64 = 0;
    let mut nested_archives = 0;

    for index in 0..entries {
        let file = archive
            .by_index_raw(index)
            .map_err(|e| format!("unreadable archive entry: {e}"))?;

        // `enclosed_name` returns None for absolute paths and `..` traversal,
        // which is exactly what a malicious archive uses.
        if file.enclosed_name().is_none() {
            return Err(format!(
                "archive contains an unsafe path: {}",
                file.name().chars().take(80).collect::<String>()
            ));
        }

        uncompressed = uncompressed.saturating_add(file.size());
        if uncompressed > MAX_UNCOMPRESSED_BYTES {
            return Err(format!(
                "archive expands to more than {} GB",
                MAX_UNCOMPRESSED_BYTES / 1024 / 1024 / 1024
            ));
        }

        let name = file.name().to_ascii_lowercase();
        if name.ends_with(".zip") || name.ends_with(".jar") || name.ends_with(".7z") {
            nested_archives += 1;
        }
    }

    let compressed = bytes.len() as u64;
    let ratio = if compressed == 0 {
        0.0
    } else {
        uncompressed as f64 / compressed as f64
    };

    if uncompressed > RATIO_CHECK_FLOOR && ratio > MAX_COMPRESSION_RATIO {
        return Err(format!(
            "archive expands {ratio:.0}x, past the {MAX_COMPRESSION_RATIO:.0}x limit; this looks like a decompression bomb"
        ));
    }

    Ok(ArchiveReport {
        entries,
        compressed,
        uncompressed,
        ratio,
        nested_archives,
    })
}

/// Best-effort content type from magic bytes, independent of the filename.
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    infer::get(bytes).map(|kind| kind.mime_type())
}

/// True when the bytes look like a native executable for any common platform.
pub fn looks_executable(bytes: &[u8]) -> bool {
    matches!(
        infer::get(bytes).map(|k| k.mime_type()),
        Some("application/vnd.microsoft.portable-executable")
            | Some("application/x-executable")
            | Some("application/x-mach-binary")
    )
}

pub fn is_archive_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".zip") || lower.ends_with(".jar")
}

// ---------------------------------------------------------------------------
// Optional virus scanning
// ---------------------------------------------------------------------------

/// Result of handing a file to ClamAV.
#[derive(Debug)]
pub enum ScanVerdict {
    Clean,
    Infected(String),
}

/// Streams bytes to a `clamd` daemon using its INSTREAM command.
///
/// Deliberately opt-in. ClamAV means running a daemon and keeping a signature
/// database current, which is a lot to demand of someone who just wants to host
/// a Minecraft server — and it would not catch a decompression bomb anyway,
/// since a bomb is not malware in the signature sense. The archive limits and
/// the filesystem sandbox do the structural work; this adds known-malware
/// coverage for operators who want it.
pub async fn scan_with_clamav(address: &str, bytes: &[u8]) -> Result<ScanVerdict, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::TcpStream::connect(address),
    )
    .await
    .map_err(|_| format!("timed out connecting to clamd at {address}"))?
    .map_err(|e| format!("could not reach clamd at {address}: {e}"))?;

    stream
        .write_all(b"zINSTREAM\0")
        .await
        .map_err(|e| format!("clamd rejected the request: {e}"))?;

    // The payload goes in length-prefixed chunks, ended by a zero length.
    for chunk in bytes.chunks(32 * 1024) {
        stream
            .write_all(&(chunk.len() as u32).to_be_bytes())
            .await
            .map_err(|e| format!("clamd write failed: {e}"))?;
        stream
            .write_all(chunk)
            .await
            .map_err(|e| format!("clamd write failed: {e}"))?;
    }
    stream
        .write_all(&0u32.to_be_bytes())
        .await
        .map_err(|e| format!("clamd write failed: {e}"))?;

    let mut reply = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(60),
        stream.read_to_end(&mut reply),
    )
    .await
    .map_err(|_| "clamd did not answer in time".to_string())?
    .map_err(|e| format!("clamd read failed: {e}"))?;

    let text = String::from_utf8_lossy(&reply).trim_end_matches('\0').to_string();

    if text.contains("FOUND") {
        let signature = text
            .rsplit_once(": ")
            .map(|(_, name)| name.trim_end_matches(" FOUND").to_string())
            .unwrap_or_else(|| text.clone());
        return Ok(ScanVerdict::Infected(signature));
    }
    if text.contains("ERROR") {
        return Err(format!("clamd reported an error: {text}"));
    }

    Ok(ScanVerdict::Clean)
}
