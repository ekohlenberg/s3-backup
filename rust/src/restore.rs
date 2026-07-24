//! Restore orchestration, replacing `Restore.run`'s
//! download/decrypt/decompress/expand shell-out chain.
//!
//! Per requirement 5.5/5.6: one object's failure aborts only that object
//! (remaining sub-steps skipped) but processing continues to the next
//! matching object; overall restore success/failure reflects both the
//! initial listing call and every per-object run. Restored files land in
//! the shared temp directory, not back at their original location -- this
//! port doesn't add auto-relocation either, matching the documented gap.

use crate::archive;
use crate::config::Config;
use crate::crypto;
use crate::error::AppError;
use crate::logging::{error, info, warn, RunSummary};
use crate::manifest::MANIFEST_KEY;
use crate::naming;
use crate::s3::S3Client;

pub fn run(
    cfg: &Config,
    bucket: &str,
    object: Option<&str>,
    private_key_path: &std::path::Path,
) -> Result<(), AppError> {
    std::fs::create_dir_all(&cfg.temp_dir).map_err(|e| AppError::io(&cfg.temp_dir, e))?;

    let client = S3Client::new(cfg, bucket);
    let private_key = crypto::load_private_key(private_key_path)?;

    // The initial listing call failing aborts the whole restore (via `?`),
    // matching "reflects... the initial listing call" in requirement 5.6.
    let objects = client.list_objects_v2(None)?;

    let matching: Vec<_> = objects
        .into_iter()
        .filter(|o| o.key != MANIFEST_KEY && !o.key.starts_with("_s3b/"))
        .filter(|o| object.map(|name| o.key == name).unwrap_or(true))
        .collect();

    if matching.is_empty() {
        match object {
            Some(name) => warn(format!("no object named '{name}' found in bucket")),
            None => warn("bucket contains no backup objects to restore"),
        }
    }

    let mut summary = RunSummary::default();
    for obj in &matching {
        match restore_object(&client, cfg, &private_key, &obj.key) {
            Ok(dest) => {
                info(format!("restored {} -> {}", obj.key, dest.display()));
                summary.succeeded += 1;
            }
            Err(e) => {
                error(format!("restore of {} failed: {e}", obj.key));
                summary.failed += 1;
            }
        }
    }
    summary.print("restore");

    if !summary.is_clean() {
        return Err(AppError::RestoreIncomplete(summary.failed));
    }
    Ok(())
}

/// Downloads, decrypts, and decompresses every backup object in the bucket
/// to verify the restore pipeline actually works end to end -- a periodic
/// self-test, not a real restore. Reuses `restore_object` (the same
/// download/decrypt/decompress/expand steps `-action restore` uses), so a
/// failure here means a real restore would fail against this same object
/// too. Always covers every object (no `-object` narrowing, unlike
/// `restore::run`) since a partial self-test wouldn't answer "does restore
/// work" -- and each object's extracted output is deleted immediately after
/// it's processed (success or failure) so nothing accumulates on disk and
/// nothing is left behind for the user to find later.
pub fn run_test(
    cfg: &Config,
    bucket: &str,
    private_key_path: &std::path::Path,
) -> Result<(), AppError> {
    std::fs::create_dir_all(&cfg.temp_dir).map_err(|e| AppError::io(&cfg.temp_dir, e))?;

    let client = S3Client::new(cfg, bucket);
    let private_key = crypto::load_private_key(private_key_path)?;

    // The initial listing call failing aborts the whole test run, same as
    // for a real restore.
    let objects = client.list_objects_v2(None)?;
    let matching: Vec<_> = objects
        .into_iter()
        .filter(|o| o.key != MANIFEST_KEY && !o.key.starts_with("_s3b/"))
        .collect();

    if matching.is_empty() {
        warn("bucket contains no backup objects to test");
    }

    let mut summary = RunSummary::default();
    for obj in &matching {
        match restore_object(&client, cfg, &private_key, &obj.key) {
            Ok(dest) => {
                info(format!("test-restore of {} succeeded", obj.key));
                cleanup_test_output(&dest);
                summary.succeeded += 1;
            }
            Err(e) => {
                error(format!("test-restore of {} failed: {e}", obj.key));
                // Best-effort: a partially-expanded directory can exist even
                // on failure (e.g. expand started before hitting a bad
                // entry). Cleaned up the same way as a success so a failed
                // test run doesn't leave more behind than a passing one.
                let base =
                    naming::base_name_from_object_key(&obj.key).unwrap_or_else(|| obj.key.replace('/', "_"));
                cleanup_test_output(&cfg.temp_dir.join(&base));
                summary.failed += 1;
            }
        }
    }
    summary.print("test");

    if !summary.is_clean() {
        return Err(AppError::TestIncomplete(summary.failed));
    }
    Ok(())
}

/// Best-effort recursive delete of a test-restore's extracted output
/// directory. Failing to clean up is logged but never treated as the run's
/// actual failure -- a stray temp directory isn't a sign restore doesn't
/// work, it's a separate, lower-stakes problem (e.g. a file still open,
/// permissions).
fn cleanup_test_output(dest: &std::path::Path) {
    match std::fs::remove_dir_all(dest) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => warn(format!(
            "could not clean up test-restore output {}: {e}",
            dest.display()
        )),
    }
}

/// Download -> decrypt -> decompress+expand for a single object. Returns as
/// soon as any sub-step fails (no silent partial success), per requirement
/// 5.5.
fn restore_object(
    client: &S3Client,
    cfg: &Config,
    private_key: &[u8; 32],
    key: &str,
) -> Result<std::path::PathBuf, AppError> {
    let base = naming::base_name_from_object_key(key).unwrap_or_else(|| key.replace('/', "_"));

    let ciphertext = client.get_object(key)?;
    let plaintext = crypto::decrypt(&ciphertext, private_key)?;

    let tar_gz_path = cfg.temp_dir.join(format!("{base}.restore.tar.gz.tmp"));
    std::fs::write(&tar_gz_path, &plaintext).map_err(|e| AppError::io(&tar_gz_path, e))?;

    let dest_dir = cfg.temp_dir.join(&base);
    let expand_result = archive::expand_tar_gz(&tar_gz_path, &dest_dir);

    if let Err(e) = std::fs::remove_file(&tar_gz_path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            warn(format!(
                "could not clean up temp file {}: {e}",
                tar_gz_path.display()
            ));
        }
    }

    expand_result?;
    Ok(dest_dir)
}
