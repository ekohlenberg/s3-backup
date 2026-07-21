//! Recipient-keypair encryption for archives: X25519 (ECDH) + HKDF-SHA256 +
//! AES-256-GCM, replacing the original shared-passphrase design (Argon2id +
//! AES-256-GCM keyed directly by one symmetric secret).
//!
//! The previous design used a single passphrase file as both the encryption
//! and decryption secret: any host that could back up could also decrypt
//! every backup ever made, from any host. With a keypair, backup machines
//! hold only the public key (`S3BPUBKEY`); decryption requires the private
//! key, which is passed explicitly via `-key` on `-action restore` and is
//! never read from the environment.
//!
//! Container format for every encrypted object (all lengths fixed, so there
//! is nothing to misparse -- unchanged in spirit from the passphrase-based
//! version):
//!
//! ```text
//! [ version: u8 = 1 ][ ephemeral_pubkey: 32 bytes ][ nonce: 12 bytes ][ AES-256-GCM ciphertext+tag ]
//! ```
//!
//! A fresh ephemeral X25519 keypair is generated per file. Its public half
//! travels in the container; the private half is used once for the ECDH
//! exchange and then dropped, so encryption never needs to hold any
//! long-term secret beyond the recipient's public key.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use hkdf::Hkdf;
use rand::RngCore;
use sha2::Sha256;
use std::path::{Path, PathBuf};
use x25519_dalek::{EphemeralSecret, PublicKey, StaticSecret};

use crate::error::AppError;
use crate::hashing::sha256_hex;
use crate::logging::info;

const VERSION: u8 = 1;
const PUBKEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const HEADER_LEN: usize = 1 + PUBKEY_LEN + NONCE_LEN;
const HKDF_INFO: &[u8] = b"s3b-file-key";

/// Env vars checked for the recipient public key path, in order -- same
/// two-name pattern the original passphrase file used (`S3BPASSFILE` /
/// `S3B-PASSFILE`).
pub const PUBKEY_ENV: &[&str] = &["S3BPUBKEY", "S3B-PUBKEY"];

/// Default `genkey` prefix when `-out` is omitted, and the basename `backup`
/// falls back to (as `~/s3b.pub`) when neither `S3BPUBKEY` nor `S3B-PUBKEY`
/// is set.
pub const DEFAULT_KEY_PREFIX: &str = "s3b";

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    rand::thread_rng().fill_bytes(&mut buf);
    buf
}

fn derive_key(shared_secret: &[u8; 32]) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(None, shared_secret);
    let mut key = [0u8; 32];
    hk.expand(HKDF_INFO, &mut key)
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    key
}

/// Encrypts `plaintext` for the holder of `recipient_public`.
///
/// Also performs a round-trip self-check (bulletproofing checklist) before
/// returning. Unlike the old passphrase design, the encrypting side never
/// holds the recipient's private key -- that's the entire point of this
/// scheme -- so the check can only re-derive from the *same* shared secret
/// just computed here, not redo the ECDH exchange independently via a
/// separate key. It still catches a local AES-GCM implementation bug (wrong
/// key/nonce/slicing) before anything reaches disk; it does not catch a bug
/// in the ECDH derivation itself, which `decrypt`'s own test coverage
/// (round-tripping through an independently generated keypair) exercises
/// instead.
pub fn encrypt(plaintext: &[u8], recipient_public: &[u8; 32]) -> Result<Vec<u8>, AppError> {
    let recipient_public = PublicKey::from(*recipient_public);

    let ephemeral_secret = EphemeralSecret::random_from_rng(rand::rngs::OsRng);
    let ephemeral_public = PublicKey::from(&ephemeral_secret);
    let shared_secret = ephemeral_secret.diffie_hellman(&recipient_public);
    let key_bytes = derive_key(shared_secret.as_bytes());

    let nonce_bytes = random_bytes::<NONCE_LEN>();
    let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
    let cipher = Aes256Gcm::new(key);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| AppError::Crypto(format!("encryption failed: {e}")))?;

    let roundtrip = cipher
        .decrypt(nonce, ciphertext.as_slice())
        .map_err(|e| AppError::Crypto(format!("round-trip self-check failed: {e}")))?;
    if sha256_hex(&roundtrip) != sha256_hex(plaintext) {
        return Err(AppError::Crypto(
            "round-trip self-check mismatch: decrypted plaintext does not match original".into(),
        ));
    }

    let mut out = Vec::with_capacity(HEADER_LEN + ciphertext.len());
    out.push(VERSION);
    out.extend_from_slice(ephemeral_public.as_bytes());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypts a container produced by [`encrypt`] using the matching private
/// key.
pub fn decrypt(container: &[u8], private_key: &[u8; 32]) -> Result<Vec<u8>, AppError> {
    if container.len() < HEADER_LEN {
        return Err(AppError::Crypto("encrypted container is truncated".into()));
    }
    let version = container[0];
    if version != VERSION {
        return Err(AppError::Crypto(format!(
            "unsupported encryption container version {version}"
        )));
    }

    let mut eph_pub_bytes = [0u8; PUBKEY_LEN];
    eph_pub_bytes.copy_from_slice(&container[1..1 + PUBKEY_LEN]);
    let ephemeral_public = PublicKey::from(eph_pub_bytes);

    let nonce_bytes = &container[1 + PUBKEY_LEN..HEADER_LEN];
    let ciphertext = &container[HEADER_LEN..];

    let secret = StaticSecret::from(*private_key);
    let shared_secret = secret.diffie_hellman(&ephemeral_public);
    let key_bytes = derive_key(shared_secret.as_bytes());

    let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
    let cipher = Aes256Gcm::new(key);
    let nonce = Nonce::from_slice(nonce_bytes);

    cipher.decrypt(nonce, ciphertext).map_err(|_| {
        AppError::Crypto("decryption failed: wrong key, or data corrupted/tampered".into())
    })
}

/// A freshly generated recipient keypair, in raw 32-byte form.
pub struct KeyPair {
    pub public: [u8; 32],
    pub private: [u8; 32],
}

pub fn generate_keypair() -> KeyPair {
    let secret = StaticSecret::random_from_rng(rand::rngs::OsRng);
    let public = PublicKey::from(&secret);
    KeyPair {
        public: *public.as_bytes(),
        private: secret.to_bytes(),
    }
}

/// `-action genkey [-out <prefix>]`: generates a keypair and writes
/// `<prefix>.pub` / `<prefix>.key` (base64-encoded raw key bytes). `prefix`
/// is `DEFAULT_KEY_PREFIX` ("s3b") when the caller (main.rs) didn't get an
/// explicit `-out`. The private key file is restricted to owner-only
/// permissions on write.
pub fn genkey(prefix: &str) -> Result<(), AppError> {
    let kp = generate_keypair();

    let pub_path = format!("{prefix}.pub");
    let key_path = format!("{prefix}.key");

    std::fs::write(&pub_path, STANDARD.encode(kp.public)).map_err(|e| AppError::io(&pub_path, e))?;
    std::fs::write(&key_path, STANDARD.encode(kp.private)).map_err(|e| AppError::io(&key_path, e))?;
    restrict_to_owner(&key_path).map_err(|e| AppError::io(&key_path, e))?;

    info(format!("wrote public key:  {pub_path}"));
    info(format!(
        "wrote private key: {key_path} (permissions restricted to owner)"
    ));
    info(format!(
        "distribute {pub_path} to every host that performs backups (export S3BPUBKEY=<path>); \
         keep {key_path} only wherever restores are actually performed"
    ));
    Ok(())
}

/// Resolves the public key path from `S3BPUBKEY`/`S3B-PUBKEY`, falling back
/// to `~/s3b.pub` (matching `genkey`'s default `-out` prefix) if neither is
/// set, and loads it. Called from the backup pipeline; `genkey` and
/// `restore` don't need it.
pub fn resolve_and_load_public_key() -> Result<[u8; 32], AppError> {
    let path = match PUBKEY_ENV.iter().find_map(|name| std::env::var(name).ok()) {
        Some(p) => PathBuf::from(p),
        None => default_pubkey_path().ok_or_else(|| {
            AppError::Config(format!(
                "no public key configured: set {} (or {}) to the path of a .pub file written by 'genkey', \
                 and no home directory could be resolved to fall back to ~/{DEFAULT_KEY_PREFIX}.pub",
                PUBKEY_ENV[0], PUBKEY_ENV[1]
            ))
        })?,
    };
    load_public_key(&path)
}

/// `~/<DEFAULT_KEY_PREFIX>.pub`, or `None` if no home directory can be
/// resolved. Hand-rolled rather than pulling in the `dirs` crate, matching
/// the "minimize dependencies" goal -- same spirit as `config::hostname_fallback`.
fn default_pubkey_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE")) // Windows
        .map(|home| PathBuf::from(home).join(format!("{DEFAULT_KEY_PREFIX}.pub")))
}

pub fn load_public_key(path: &Path) -> Result<[u8; 32], AppError> {
    let text = std::fs::read_to_string(path).map_err(|e| AppError::io(path, e))?;
    decode_key(text.trim(), path)
}

/// Loads the private key from `path` (the `-key` CLI flag on restore),
/// refusing to use it if the file is readable by anyone but its owner --
/// the same posture OpenSSH takes toward `~/.ssh/id_*` files.
pub fn load_private_key(path: &Path) -> Result<[u8; 32], AppError> {
    check_private_key_permissions(path)?;
    let text = std::fs::read_to_string(path).map_err(|e| AppError::io(path, e))?;
    decode_key(text.trim(), path)
}

fn decode_key(text: &str, path: &Path) -> Result<[u8; 32], AppError> {
    let bytes = STANDARD.decode(text).map_err(|e| {
        AppError::Crypto(format!("key file {} is not valid base64: {e}", path.display()))
    })?;
    bytes.try_into().map_err(|v: Vec<u8>| {
        AppError::Crypto(format!(
            "key file {} must decode to 32 bytes, got {}",
            path.display(),
            v.len()
        ))
    })
}

#[cfg(unix)]
fn restrict_to_owner(path: &str) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict_to_owner(_path: &str) -> std::io::Result<()> {
    // TODO: apply an equivalent owner-only ACL on Windows (icacls) before
    // this ships there. macOS/Linux are s3b's other stated target platforms
    // and are handled above via POSIX permission bits.
    Ok(())
}

#[cfg(unix)]
fn check_private_key_permissions(path: &Path) -> Result<(), AppError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .map_err(|e| AppError::io(path, e))?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        return Err(AppError::Crypto(format!(
            "refusing to use private key '{}': permissions {:o} are readable by group/other; run `chmod 600 {}`",
            path.display(),
            mode & 0o777,
            path.display()
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_private_key_permissions(_path: &Path) -> Result<(), AppError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let kp = generate_keypair();
        let plaintext = b"the quick brown fox jumps over the lazy dog";
        let ct = encrypt(plaintext, &kp.public).unwrap();
        let pt = decrypt(&ct, &kp.private).unwrap();
        assert_eq!(pt, plaintext);
    }

    #[test]
    fn wrong_key_fails() {
        let kp_a = generate_keypair();
        let kp_b = generate_keypair();
        let ct = encrypt(b"secret data", &kp_a.public).unwrap();
        let err = decrypt(&ct, &kp_b.private).unwrap_err();
        assert!(matches!(err, AppError::Crypto(_)));
    }

    #[test]
    fn tampering_is_detected() {
        // This is the whole point of using GCM: corruption/tampering must be
        // detected on decrypt.
        let kp = generate_keypair();
        let plaintext = b"data that must not be silently corrupted";
        let mut ct = encrypt(plaintext, &kp.public).unwrap();
        let last = ct.len() - 1;
        ct[last] ^= 0xFF; // flip a bit in the auth tag
        let err = decrypt(&ct, &kp.private).unwrap_err();
        assert!(matches!(err, AppError::Crypto(_)));
    }

    #[test]
    fn each_encryption_uses_a_fresh_ephemeral_key_and_nonce() {
        let kp = generate_keypair();
        let plaintext = b"same plaintext twice";
        let ct1 = encrypt(plaintext, &kp.public).unwrap();
        let ct2 = encrypt(plaintext, &kp.public).unwrap();
        assert_ne!(
            ct1, ct2,
            "identical plaintext must not produce identical ciphertext"
        );
    }

    #[test]
    fn empty_plaintext_round_trips() {
        let kp = generate_keypair();
        let ct = encrypt(b"", &kp.public).unwrap();
        let pt = decrypt(&ct, &kp.private).unwrap();
        assert_eq!(pt, b"");
    }

    #[test]
    fn truncated_container_is_rejected() {
        let kp = generate_keypair();
        let err = decrypt(&[0u8; 10], &kp.private).unwrap_err();
        assert!(matches!(err, AppError::Crypto(_)));
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let kp = generate_keypair();
        let mut ct = encrypt(b"hello", &kp.public).unwrap();
        ct[0] = 99;
        let err = decrypt(&ct, &kp.private).unwrap_err();
        assert!(matches!(err, AppError::Crypto(_)));
    }

    // Both env-mutating checks live in one test (rather than two) to avoid a
    // parallel-test race on the process-global HOME/S3BPUBKEY/S3B-PUBKEY env
    // vars; original values are restored afterward regardless of outcome.
    #[test]
    fn default_pubkey_path_and_env_fallback() {
        let saved: Vec<(&str, Option<String>)> = ["HOME", "S3BPUBKEY", "S3B-PUBKEY"]
            .iter()
            .map(|n| (*n, std::env::var(n).ok()))
            .collect();

        let restore = || {
            for (name, value) in &saved {
                match value {
                    Some(v) => std::env::set_var(name, v),
                    None => std::env::remove_var(name),
                }
            }
        };

        let result = std::panic::catch_unwind(|| {
            let home = tempfile::tempdir().unwrap();
            std::env::set_var("HOME", home.path());
            std::env::remove_var("S3BPUBKEY");
            std::env::remove_var("S3B-PUBKEY");

            // default_pubkey_path() resolves to $HOME/s3b.pub without
            // requiring the file to exist yet.
            let expected = home.path().join("s3b.pub");
            assert_eq!(default_pubkey_path(), Some(expected.clone()));

            // resolve_and_load_public_key() actually reads it: write a
            // genkey-shaped .pub file there and confirm the fallback loads it.
            let kp = generate_keypair();
            std::fs::write(&expected, STANDARD.encode(kp.public)).unwrap();
            let loaded = resolve_and_load_public_key().unwrap();
            assert_eq!(loaded, kp.public);
        });

        restore();
        result.unwrap();
    }
}
