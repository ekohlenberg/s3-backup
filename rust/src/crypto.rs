//! Encryption for archives, replacing the original `openssl enc` AES-256-CBC
//! shell-out with in-process AES-256-GCM (RustCrypto `aes-gcm`), and raw
//! passphrase-file bytes with an Argon2id-derived key.
//!
//! Container format for every encrypted object (all integers little-endian,
//! all lengths fixed so there is nothing to misparse):
//!
//! ```text
//! [ version: u8 = 1 ][ salt: 16 bytes ][ nonce: 12 bytes ][ AES-256-GCM ciphertext+tag ]
//! ```
//!
//! The salt is generated fresh per encryption and travels with the
//! ciphertext, so the file is self-describing -- decryption never depends on
//! any external state surviving. The version byte exists so a future format
//! change (different KDF params, different AEAD) can be introduced without
//! breaking the ability to decrypt older archives, per the "key rotation"
//! goal in the migration notes.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use argon2::Argon2;
use rand::RngCore;

use crate::error::AppError;
use crate::hashing::sha256_hex;

const VERSION: u8 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

fn derive_key(passphrase_bytes: &[u8], salt: &[u8]) -> Result<[u8; KEY_LEN], AppError> {
    let argon2 = Argon2::default(); // Argon2id, latest version, default (memory-hard) params
    let mut key = [0u8; KEY_LEN];
    argon2
        .hash_password_into(passphrase_bytes, salt, &mut key)
        .map_err(|e| AppError::Crypto(format!("key derivation failed: {e}")))?;
    Ok(key)
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    rand::thread_rng().fill_bytes(&mut buf);
    buf
}

/// Encrypts `plaintext` under a key derived from `passphrase_bytes`, then
/// immediately decrypts the result in memory and compares it against the
/// original -- the "round-trip self-check" from the bulletproofing
/// checklist, catching a local encryption bug before it ever reaches disk or
/// the network.
pub fn encrypt(plaintext: &[u8], passphrase_bytes: &[u8]) -> Result<Vec<u8>, AppError> {
    let salt = random_bytes::<SALT_LEN>();
    let nonce_bytes = random_bytes::<NONCE_LEN>();

    let key_bytes = derive_key(passphrase_bytes, &salt)?;
    let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
    let cipher = Aes256Gcm::new(key);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| AppError::Crypto(format!("encryption failed: {e}")))?;

    let mut out = Vec::with_capacity(1 + SALT_LEN + NONCE_LEN + ciphertext.len());
    out.push(VERSION);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);

    // Round-trip self-check before this ever leaves the function.
    let roundtrip = decrypt(&out, passphrase_bytes)
        .map_err(|e| AppError::Crypto(format!("round-trip self-check failed: {e}")))?;
    if sha256_hex(&roundtrip) != sha256_hex(plaintext) {
        return Err(AppError::Crypto(
            "round-trip self-check mismatch: decrypted plaintext does not match original".into(),
        ));
    }

    Ok(out)
}

pub fn decrypt(container: &[u8], passphrase_bytes: &[u8]) -> Result<Vec<u8>, AppError> {
    if container.len() < 1 + SALT_LEN + NONCE_LEN {
        return Err(AppError::Crypto("encrypted container is truncated".into()));
    }
    let version = container[0];
    if version != VERSION {
        return Err(AppError::Crypto(format!(
            "unsupported encryption container version {version}"
        )));
    }
    let salt = &container[1..1 + SALT_LEN];
    let nonce_bytes = &container[1 + SALT_LEN..1 + SALT_LEN + NONCE_LEN];
    let ciphertext = &container[1 + SALT_LEN + NONCE_LEN..];

    let key_bytes = derive_key(passphrase_bytes, salt)?;
    let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
    let cipher = Aes256Gcm::new(key);
    let nonce = Nonce::from_slice(nonce_bytes);

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| AppError::Crypto("decryption failed: wrong passphrase or corrupted/tampered data".into()))
}

/// Reads the passphrase file's raw bytes to use as Argon2id input, matching
/// the requirements doc's description of how the passphrase file is
/// consumed (just its contents, provisioned out-of-band).
pub fn read_passphrase(path: &std::path::Path) -> Result<Vec<u8>, AppError> {
    std::fs::read(path).map_err(|e| AppError::io(path, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let pass = b"correct horse battery staple";
        let plaintext = b"the quick brown fox jumps over the lazy dog";
        let ct = encrypt(plaintext, pass).unwrap();
        let pt = decrypt(&ct, pass).unwrap();
        assert_eq!(pt, plaintext);
    }

    #[test]
    fn wrong_passphrase_fails() {
        let plaintext = b"secret data";
        let ct = encrypt(plaintext, b"passphrase-one").unwrap();
        let err = decrypt(&ct, b"passphrase-two").unwrap_err();
        assert!(matches!(err, AppError::Crypto(_)));
    }

    #[test]
    fn tampering_is_detected() {
        // This is the whole point of moving to GCM: corruption/tampering
        // must be detected on decrypt, per the migration notes.
        let plaintext = b"data that must not be silently corrupted";
        let pass = b"pw";
        let mut ct = encrypt(plaintext, pass).unwrap();
        let last = ct.len() - 1;
        ct[last] ^= 0xFF; // flip a bit in the auth tag
        let err = decrypt(&ct, pass).unwrap_err();
        assert!(matches!(err, AppError::Crypto(_)));
    }

    #[test]
    fn each_encryption_uses_a_fresh_salt_and_nonce() {
        let plaintext = b"same plaintext twice";
        let pass = b"pw";
        let ct1 = encrypt(plaintext, pass).unwrap();
        let ct2 = encrypt(plaintext, pass).unwrap();
        assert_ne!(ct1, ct2, "identical plaintext must not produce identical ciphertext");
    }

    #[test]
    fn empty_plaintext_round_trips() {
        let ct = encrypt(b"", b"pw").unwrap();
        let pt = decrypt(&ct, b"pw").unwrap();
        assert_eq!(pt, b"");
    }
}
