//! Backup orchestration, replacing `Backup.run` / the five-stage
//! archive/compress/encrypt/upload/clean pipeline plus the separate
//! reconciliation pass.
//!
//! Key differences from the .NET version, per the migration notes:
//! - Change detection is by content-hash of a `(path, size, mtime)`
//!   manifest, not folder-level `LastWriteTime`.
//! - There's no local database: "already backed up and current" is answered
//!   by a HEAD request reading the previous run's `source-hash` metadata.
//! - Upload verification is inline (via the PUT response's ETag), not a
//!   separate `aws s3 ls` reconciliation pass after the fact.
//! - A folder is never considered backed up until verification succeeds --
//!   fail closed, per the bulletproofing checklist.
//! - `-force` bypasses the content-hash change check so every folder is
//!   re-archived/re-encrypted/re-uploaded regardless of the recorded
//!   `source-hash`, for cases like re-keying after `genkey` or wanting a
//!   fresh verified copy without waiting for a real content change.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::archive;
use crate::config::Config;
use crate::crypto;
use crate::error::AppError;
use crate::hashing;
use crate::logging::{error, info, warn, RunSummary};
use crate::manifest::Manifest;
use crate::naming;
use crate::s3::S3Client;
use crate::time_util::iso8601_now;

enum ProcessOutcome {
    Uploaded,
    Unchanged,
}

pub fn run(cfg: &Config, folder: &str, bucket: &str, force: bool) -> Result<(), AppError> {
    let root = std::fs::canonicalize(folder).map_err(|e| AppError::io(folder, e))?;
    if !root.is_dir() {
        return Err(AppError::Config(format!(
            "-folder '{}' is not a directory",
            root.display()
        )));
    }

    std::fs::create_dir_all(&cfg.temp_dir).map_err(|e| AppError::io(&cfg.temp_dir, e))?;

    if force {
        info("-force set: skipping the content-hash change check, re-uploading every folder");
    }

    let client = S3Client::new(cfg, bucket);
    let public_key = crypto::resolve_and_load_public_key()?;
    let mut manifest = Manifest::load(&client)?;

    // Immediate child directories (recursive scan each) plus the root
    // itself (non-recursive: only files directly in the root, so its
    // subfolders' contents aren't counted twice) -- matches requirement 4.3.
    let mut pending: Vec<(PathBuf, bool)> = Vec::new();
    for entry in std::fs::read_dir(&root).map_err(|e| AppError::io(&root, e))? {
        let entry = entry.map_err(|e| AppError::io(&root, e))?;
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            pending.push((entry.path(), true));
        }
    }
    pending.push((root.clone(), false));

    let mut summary = RunSummary::default();
    let max_attempts = cfg.retry_attempts + 1;

    for attempt in 1..=max_attempts {
        if pending.is_empty() {
            break;
        }
        info(format!(
            "backup attempt {attempt}/{max_attempts}: {} folder(s) to process",
            pending.len()
        ));

        let mut still_failing = Vec::new();
        for (path, recursive) in pending.drain(..) {
            match process_folder(&client, cfg, &public_key, &path, recursive, force) {
                Ok(ProcessOutcome::Uploaded) => {
                    info(format!("uploaded {}", path.display()));
                    summary.succeeded += 1;
                }
                Ok(ProcessOutcome::Unchanged) => {
                    info(format!("unchanged, skipping {}", path.display()));
                    summary.skipped_unchanged += 1;
                }
                Err(e) => {
                    error(format!("folder {} failed: {e}", path.display()));
                    still_failing.push((path, recursive));
                }
            }
        }
        pending = still_failing;

        if !pending.is_empty() && attempt < max_attempts {
            let backoff_secs = 2u64.saturating_pow(attempt.min(5));
            warn(format!(
                "{} folder(s) failed verification; retrying after {backoff_secs}s ({}/{} attempts used)",
                pending.len(),
                attempt,
                max_attempts
            ));
            std::thread::sleep(Duration::from_secs(backoff_secs));
        }
    }

    summary.failed = pending.len();
    summary.print("backup");

    // The backup set's last_backup_datetime is stamped on completion
    // regardless of whether any folders remained in error, matching
    // requirement 4.8.
    manifest.upsert(
        &root.to_string_lossy(),
        &cfg.hostname,
        &cfg.username,
        &iso8601_now(),
    );
    manifest.save(&client)?;

    if !summary.is_clean() {
        for (path, _) in &pending {
            error(format!("giving up on {} after {max_attempts} attempt(s)", path.display()));
        }
        return Err(AppError::BackupIncomplete(summary.failed));
    }
    Ok(())
}

fn process_folder(
    client: &S3Client,
    cfg: &Config,
    public_key: &[u8; 32],
    folder: &Path,
    recursive: bool,
    force: bool,
) -> Result<ProcessOutcome, AppError> {
    let folder_path_str = folder.to_string_lossy().to_string();
    let local_hash = hashing::hash_folder(folder, recursive)?;
    let object_key = naming::object_key(&cfg.hostname, &cfg.username, &folder_path_str);

    // -force skips this entirely -- no HEAD request, no comparison -- so a
    // forced run always re-archives/re-encrypts/re-uploads every folder.
    if !force {
        if let Some(existing) = client.head_object(&object_key)? {
            if existing.source_hash.as_deref() == Some(local_hash.as_str()) {
                return Ok(ProcessOutcome::Unchanged);
            }
        }
    }

    // Archive + compress, streaming straight to a temp file (single
    // intermediate file, atomically renamed into place by `create_tar_gz`).
    let tar_gz_path = cfg.temp_dir.join(format!("{object_key}.tar.gz.tmp"));
    archive::create_tar_gz(folder, recursive, &tar_gz_path)?;

    let cleanup_tar_gz = |path: &Path| {
        if let Err(e) = std::fs::remove_file(path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn(format!("could not clean up temp file {}: {e}", path.display()));
            }
        }
    };

    let plaintext_result = std::fs::read(&tar_gz_path);
    let plaintext = match plaintext_result {
        Ok(bytes) => bytes,
        Err(e) => {
            cleanup_tar_gz(&tar_gz_path);
            return Err(AppError::io(&tar_gz_path, e));
        }
    };

    let ciphertext = match crypto::encrypt(&plaintext, public_key) {
        Ok(ct) => ct,
        Err(e) => {
            cleanup_tar_gz(&tar_gz_path);
            return Err(e);
        }
    };

    // Persist the encrypted archive to disk too (atomic .tmp + rename)
    // before uploading, so a crash between encrypt and upload leaves a
    // recoverable artifact rather than nothing -- the upload itself still
    // reads from the in-memory buffer we already have.
    let enc_path = cfg.temp_dir.join(format!("{object_key}.enc"));
    let enc_tmp_path = cfg.temp_dir.join(format!("{object_key}.enc.tmp"));
    if let Err(e) = std::fs::write(&enc_tmp_path, &ciphertext) {
        cleanup_tar_gz(&tar_gz_path);
        return Err(AppError::io(&enc_tmp_path, e));
    }
    if let Err(e) = std::fs::rename(&enc_tmp_path, &enc_path) {
        cleanup_tar_gz(&tar_gz_path);
        return Err(AppError::io(&enc_path, e));
    }

    let backup_time = iso8601_now();
    let metadata = [
        ("source-hash", local_hash.as_str()),
        ("source-path", folder_path_str.as_str()),
        ("hostname", cfg.hostname.as_str()),
        ("username", cfg.username.as_str()),
        ("backup-time", backup_time.as_str()),
    ];

    info(format!(
        "uploading {} -> {object_key} ({} bytes)",
        folder.display(),
        ciphertext.len()
    ));
    let upload_result = client.put_object(&object_key, &ciphertext, &metadata);

    // Clean up local temp files regardless of upload outcome -- the
    // archive/tar temp file cleanup is intentional (per requirement 4.5);
    // the encrypted temp file is removed too once the upload attempt (not
    // necessarily success) has been made, since it isn't needed for a retry
    // (a retry re-derives it from source, matching "no resumable mid-
    // pipeline state" from the migration notes).
    cleanup_tar_gz(&tar_gz_path);
    if let Err(e) = std::fs::remove_file(&enc_path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            warn(format!("could not clean up temp file {}: {e}", enc_path.display()));
        }
    }

    let put_result = upload_result?;

    // Verify before considering this folder backed up. Plain 32-hex-char
    // ETags are the MD5 of the uploaded bytes for a single-part PUT with no
    // server-side encryption; some S3-compatible providers or bucket-level
    // SSE configurations return a different ETag shape we can't independently
    // recompute, so we only *reject* on a definite mismatch and otherwise
    // accept a non-empty ETag as confirmation the object exists.
    if put_result.etag.len() == 32 && put_result.etag.chars().all(|c| c.is_ascii_hexdigit()) {
        let expected = hashing::md5_hex(&ciphertext);
        if put_result.etag != expected {
            return Err(AppError::S3(format!(
                "upload verification failed for {object_key}: expected ETag {expected}, got {}",
                put_result.etag
            )));
        }
    } else if put_result.etag.is_empty() {
        return Err(AppError::S3(format!(
            "upload verification failed for {object_key}: no ETag returned"
        )));
    }

    Ok(ProcessOutcome::Uploaded)
}
