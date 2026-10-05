// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
//! Encryption for the few secrets the panel must be able to read back.
//!
//! Passwords and recovery codes are hashed, because the panel only ever needs
//! to *check* them. A TOTP secret is different: deriving the expected six-digit
//! code requires the original value, so it cannot be hashed. Instead it is
//! encrypted under a key kept in a file beside the database, which means a
//! stolen copy of `panel.db` alone is not enough to mint codes.

use crate::error::{AppError, AppResult};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use std::path::Path;

const KEY_FILE: &str = "secret.key";
const VERSION: &str = "v1";

/// Loads the panel's encryption key, creating it on first run.
pub fn load_or_create_key(data_dir: &Path) -> AppResult<[u8; 32]> {
    let path = data_dir.join(KEY_FILE);

    if let Ok(existing) = std::fs::read(&path) {
        if existing.len() == 32 {
            let mut key = [0u8; 32];
            key.copy_from_slice(&existing);
            return Ok(key);
        }
        return Err(AppError::Other(anyhow::anyhow!(
            "{} is not a valid key file; move it aside to have a new one generated",
            path.display()
        )));
    }

    let mut key = [0u8; 32];
    argon2::password_hash::rand_core::RngCore::fill_bytes(
        &mut argon2::password_hash::rand_core::OsRng,
        &mut key,
    );
    std::fs::write(&path, key)?;
    restrict_permissions(&path);

    tracing::info!(path = %path.display(), "generated an encryption key");
    Ok(key)
}

/// Makes the key file owner-only where the platform supports it.
#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
        tracing::warn!(error = %e, "could not restrict permissions on the key file");
    }
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {
    // Windows inherits the data directory's ACL, which is already user-scoped
    // for a normal install.
}

/// Encrypts to `v1:<nonce hex>:<ciphertext hex>`.
pub fn encrypt(key: &[u8; 32], plaintext: &str) -> AppResult<String> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));

    // A fresh random nonce per value; never reused for a different secret.
    let mut nonce_bytes = [0u8; 12];
    argon2::password_hash::rand_core::RngCore::fill_bytes(
        &mut argon2::password_hash::rand_core::OsRng,
        &mut nonce_bytes,
    );
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|_| AppError::Other(anyhow::anyhow!("could not encrypt the value")))?;

    Ok(format!(
        "{VERSION}:{}:{}",
        hex::encode(nonce_bytes),
        hex::encode(ciphertext)
    ))
}

/// Decrypts a value produced by [`encrypt`].
///
/// Values that are not in the versioned format are returned unchanged, so
/// secrets written before encryption existed keep working and are re-encrypted
/// the next time they are saved.
pub fn decrypt(key: &[u8; 32], stored: &str) -> AppResult<String> {
    let Some(rest) = stored.strip_prefix(&format!("{VERSION}:")) else {
        return Ok(stored.to_string());
    };

    let (nonce_hex, cipher_hex) = rest
        .split_once(':')
        .ok_or_else(|| AppError::Other(anyhow::anyhow!("stored secret is malformed")))?;

    let nonce_bytes = hex::decode(nonce_hex)
        .map_err(|_| AppError::Other(anyhow::anyhow!("stored secret has a bad nonce")))?;
    let ciphertext = hex::decode(cipher_hex)
        .map_err(|_| AppError::Other(anyhow::anyhow!("stored secret is not readable")))?;

    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let plaintext = cipher
        .decrypt(Nonce::from_slice(&nonce_bytes), ciphertext.as_ref())
        .map_err(|_| {
            AppError::Other(anyhow::anyhow!(
                "could not decrypt a stored secret; the key file may have been replaced"
            ))
        })?;

    String::from_utf8(plaintext)
        .map_err(|_| AppError::Other(anyhow::anyhow!("decrypted secret is not valid text")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let key = [7u8; 32];
        let encrypted = encrypt(&key, "hello").unwrap();
        assert!(encrypted.starts_with("v1:"));
        assert_eq!(decrypt(&key, &encrypted).unwrap(), "hello");
    }

    #[test]
    fn passes_through_legacy_plaintext() {
        let key = [7u8; 32];
        assert_eq!(decrypt(&key, "abc123").unwrap(), "abc123");
    }

    #[test]
    fn rejects_a_different_key() {
        let encrypted = encrypt(&[1u8; 32], "hello").unwrap();
        assert!(decrypt(&[2u8; 32], &encrypted).is_err());
    }
}
