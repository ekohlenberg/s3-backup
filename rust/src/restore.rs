//! Restore orchestration, replacing `Restore.run`'s
//! download/decrypt/decompress/expand shell-out chain.
//!
//! Per requirement 5.5/5.6: one object's failure aborts only that object
//! (remaining sub-steps skipped) but processing continues to the next
//! matching object; overall restore success/failure reflects both the
//! initial listing call and every per-object run.
//!
//! Restored files land in `<temp_dir>/restore/<base>/`, not back at their
//! original location -- this port doesn't add auto-relocation either,
//! matching the documented gap. Unlike the rest of `temp_dir`, `restore/`
//! survives `Config::reset_temp_dir` (see that function), which is what
//! makes `restore_one`'s skip-if-already-restored logic below meaningful: a
//! rerun of `-action restore` after a partial failure (a dropped connection,
//! a DNS blip -- see the transport errors that motivated this) only
//! re-downloads/re-decrypts/re-expands objects that didn't finish last time,
//! rather than starting the whole bucket over.

use std::path::{Path, PathBuf};

use crate::archive;
use crate::config::Config;
use crate::crypto;
use crate::error::AppError;
use crate::logging::{error, info, warn, RunSummary};
use crate::manifest::MANIFEST_KEY;
use crate::naming;
use crate::s3::S3Client;

/// Marks a restore object's output directory as complete, storing the
/// `source-hash` object metadata value that was current as of that restore.
/// A plain text file, not JSON: the only thing ever stored is one hash
/// string, so a small parser/serde struct would be pure overhead.
const COMPLETE_MARKER_FILENAME: &str = ".s3b-restore-complete";

pub fn run(
    cfg: &Config,
    bucket: &str,
    object: Option<&str>,
    private_key_path: &Path,
    force: bool,
) -> Result<(), AppError> {
    std::fs::create_dir_all(&cfg.temp_dir).map_err(|e| AppError::io(&cfg.temp_dir, e))?;
    let restore_root = cfg.restore_dir();
    std::fs::create_dir_all(&restore_root).map_err(|e| AppError::io(&restore_root, e))?;

    if force {
        info("-force set: ignoring any previously restored output, re-downloading every object");
    }

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
        match restore_one(&client, cfg, &private_key, &obj.key, force) {
            Ok(RestoreOutcome::Restored(dest)) => {
                info(format!("restored {} -> {}", obj.key, dest.display()));
                summary.succeeded += 1;
            }
            Ok(RestoreOutcome::Skipped(dest)) => {
                info(format!(
                    "{} already restored and unchanged, skipping -> {}",
                    obj.key,
                    dest.display()
                ));
                summary.skipped_unchanged += 1;
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
///
/// Deliberately bypasses the `restore/` persistent-output/skip machinery
/// `restore::run` uses: a self-test is only meaningful if it actually
/// exercises the full pipeline every time, and its output goes directly
/// under `temp_dir` (not `temp_dir/restore/`) so it's cleaned up here and
/// also swept by the next `reset_temp_dir` if this run is interrupted before
/// cleanup runs.
pub fn run_test(
    cfg: &Config,
    bucket: &str,
    private_key_path: &Path,
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
        let base = naming::base_name_from_object_key(&obj.key).unwrap_or_else(|| obj.key.replace('/', "_"));
        let dest = cfg.temp_dir.join(&base);
        match restore_object(&client, cfg, &private_key, &obj.key, &dest) {
            Ok(()) => {
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
                cleanup_test_output(&dest);
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
fn cleanup_test_output(dest: &Path) {
    match std::fs::remove_dir_all(dest) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => warn(format!(
            "could not clean up test-restore output {}: {e}",
            dest.display()
        )),
    }
}

enum RestoreOutcome {
    Restored(PathBuf),
    Skipped(PathBuf),
}

/// Restores one object into its persistent output directory under
/// `cfg.restore_dir()`, skipping the download/decrypt/expand entirely when a
/// previous run already restored this exact object -- same `source-hash`
/// object metadata as last time -- and `-force` wasn't given.
///
/// This is what makes rerunning `-action restore` after a partial failure
/// cheap: objects that already finished are a no-op; only objects that
/// didn't finish (or whose source has since changed) do any work. The
/// completion marker is only written *after* `restore_object` succeeds, so a
/// run that dies mid-object (a dropped connection, a DNS failure) leaves
/// that one object markerless -- the next run correctly redoes just that
/// object, not the ones that already completed.
fn restore_one(
    client: &S3Client,
    cfg: &Config,
    private_key: &[u8; 32],
    key: &str,
    force: bool,
) -> Result<RestoreOutcome, AppError> {
    let base = naming::base_name_from_object_key(key).unwrap_or_else(|| key.replace('/', "_"));
    let dest_dir = cfg.restore_dir().join(&base);

    // HEAD once up front: used both to decide whether to skip below and, if
    // this object does get restored, to stamp the fresh completion marker --
    // so a skip costs one lightweight request and a real restore doesn't pay
    // for a second one later just to re-read the same metadata.
    let source_hash = client.head_object(key)?.and_then(|m| m.source_hash);

    if !force && is_unchanged(read_complete_marker(&dest_dir).as_deref(), source_hash.as_deref()) {
        return Ok(RestoreOutcome::Skipped(dest_dir));
    }

    if source_hash.is_none() {
        warn(format!(
            "{key} has no source-hash metadata (uploaded by an older version, or by a tool that \
             doesn't set it?) -- freshness can't be verified, so a rerun will always re-restore this \
             object regardless of -force"
        ));
    }

    // Clear out anything left over from a prior incomplete or now-stale
    // restore of this object before expanding into it again -- otherwise a
    // partial previous expand (or content the new archive no longer
    // contains) could linger alongside the fresh output.
    if dest_dir.exists() {
        std::fs::remove_dir_all(&dest_dir).map_err(|e| AppError::io(&dest_dir, e))?;
    }

    restore_object(client, cfg, private_key, key, &dest_dir)?;

    if let Some(hash) = &source_hash {
        write_complete_marker(&dest_dir, hash)?;
    }

    Ok(RestoreOutcome::Restored(dest_dir))
}

/// True when `marker` (the source-hash recorded by a previous successful
/// restore of this object, if any) matches `current` (the object's
/// source-hash metadata right now). `None` on either side (no prior restore,
/// or no metadata to compare against) is never "unchanged" -- restore always
/// proceeds when freshness can't be positively confirmed, per the
/// fail-closed philosophy in the migration notes.
fn is_unchanged(marker: Option<&str>, current: Option<&str>) -> bool {
    matches!((marker, current), (Some(m), Some(c)) if m == c)
}

/// Reads the completion marker left in `dest_dir` by a previous successful
/// restore, if any. Missing file, missing directory, or any other read
/// error is all treated the same way here (no prior successful restore to
/// trust) rather than surfaced as a failure -- restore always has a safe
/// fallback (do the work) when this can't be read.
fn read_complete_marker(dest_dir: &Path) -> Option<String> {
    std::fs::read_to_string(dest_dir.join(COMPLETE_MARKER_FILENAME))
        .ok()
        .map(|s| s.trim().to_string())
}

/// Writes `source_hash` to `dest_dir`'s completion marker, atomically
/// (`.tmp` + rename) so a crash mid-write can never leave a marker file that
/// looks present but holds truncated/partial content.
fn write_complete_marker(dest_dir: &Path, source_hash: &str) -> Result<(), AppError> {
    let marker_path = dest_dir.join(COMPLETE_MARKER_FILENAME);
    let tmp_path = dest_dir.join(format!("{COMPLETE_MARKER_FILENAME}.tmp"));
    std::fs::write(&tmp_path, source_hash).map_err(|e| AppError::io(&tmp_path, e))?;
    std::fs::rename(&tmp_path, &marker_path).map_err(|e| AppError::io(&marker_path, e))
}

/// Download -> decrypt -> decompress+expand for a single object into
/// `dest_dir`. Returns as soon as any sub-step fails (no silent partial
/// success), per requirement 5.5.
fn restore_object(
    client: &S3Client,
    cfg: &Config,
    private_key: &[u8; 32],
    key: &str,
    dest_dir: &Path,
) -> Result<(), AppError> {
    let base = naming::base_name_from_object_key(key).unwrap_or_else(|| key.replace('/', "_"));

    // Chunked + per-chunk-retried above cfg.multipart_threshold_bytes -- see
    // S3Client::download_object. This is what makes restoring a large media
    // folder (many GiB in one object) survive a connection that can't stay
    // open for the whole transfer: each chunk is small enough to reliably
    // finish, and only the interrupted chunk gets retried, not the whole
    // object from byte zero.
    let ciphertext = client.download_object(
        key,
        cfg.multipart_threshold_bytes as usize,
        cfg.multipart_part_size_bytes as usize,
        cfg.multipart_part_retry_attempts,
    )?;
    let plaintext = crypto::decrypt(&ciphertext, private_key)?;

    let tar_gz_path = cfg.temp_dir.join(format!("{base}.restore.tar.gz.tmp"));
    std::fs::write(&tar_gz_path, &plaintext).map_err(|e| AppError::io(&tar_gz_path, e))?;

    let expand_result = archive::expand_tar_gz(&tar_gz_path, dest_dir);

    if let Err(e) = std::fs::remove_file(&tar_gz_path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            warn(format!(
                "could not clean up temp file {}: {e}",
                tar_gz_path.display()
            ));
        }
    }

    expand_result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_unchanged_true_only_when_both_present_and_equal() {
        assert!(is_unchanged(Some("abc"), Some("abc")));
        assert!(!is_unchanged(Some("abc"), Some("def")));
        assert!(!is_unchanged(None, Some("abc")), "no prior marker -- must restore");
        assert!(!is_unchanged(Some("abc"), None), "no current metadata -- must restore");
        assert!(!is_unchanged(None, None));
    }

    #[test]
    fn complete_marker_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_complete_marker(dir.path()), None, "no marker written yet");

        write_complete_marker(dir.path(), "deadbeef").unwrap();
        assert_eq!(read_complete_marker(dir.path()).as_deref(), Some("deadbeef"));

        // A later restore updates the marker in place.
        write_complete_marker(dir.path(), "newhash").unwrap();
        assert_eq!(read_complete_marker(dir.path()).as_deref(), Some("newhash"));

        assert!(
            !dir.path().join(format!("{COMPLETE_MARKER_FILENAME}.tmp")).exists(),
            "tmp file should be renamed away, not left behind"
        );
    }

    #[test]
    fn read_complete_marker_missing_directory_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        assert_eq!(read_complete_marker(&missing), None);
    }
}
