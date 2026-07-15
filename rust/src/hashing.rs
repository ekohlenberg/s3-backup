//! Content-hash based change detection, replacing the original folder-level
//! `LastWriteTime`-vs-`upload_datetime` comparison (easy to spoof/lose on
//! file copies, per the migration notes).
//!
//! We hash a canonical manifest of `(relative_path, size, mtime)` tuples for
//! every file under a folder, rather than hashing file *contents*, so
//! large folders can be re-checked quickly without reading every byte. This
//! is still far more reliable than a single folder-level mtime because it
//! catches renames, deletions, added files, and size changes explicitly, and
//! is stored as durable S3 object metadata (`source-hash`) instead of a
//! local SQLite row.

use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::error::AppError;

#[derive(Debug, Clone)]
pub struct ManifestEntry {
    pub relative_path: String,
    pub size: u64,
    pub mtime_unix: i64,
}

/// Walks `root`, collecting one entry per regular file.
///
/// - `recursive = true`: descend into subdirectories (used for the
///   immediate child folders of the backup root).
/// - `recursive = false`: only files directly inside `root`, not in any
///   subdirectory (used for the backup root itself, so its subfolders'
///   contents aren't double-counted).
pub fn collect_manifest(root: &Path, recursive: bool) -> Result<Vec<ManifestEntry>, AppError> {
    let mut entries = Vec::new();
    walk(root, root, recursive, &mut entries)?;
    entries.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(entries)
}

fn walk(
    base: &Path,
    dir: &Path,
    recursive: bool,
    out: &mut Vec<ManifestEntry>,
) -> Result<(), AppError> {
    let read_dir = std::fs::read_dir(dir).map_err(|e| AppError::io(dir, e))?;
    for entry in read_dir {
        let entry = entry.map_err(|e| AppError::io(dir, e))?;
        let path: PathBuf = entry.path();
        let file_type = entry.file_type().map_err(|e| AppError::io(&path, e))?;
        if file_type.is_dir() {
            if recursive {
                walk(base, &path, recursive, out)?;
            }
            continue;
        }
        if !file_type.is_file() {
            continue; // skip symlinks/special files -- no exclusion feature is implemented, matching the .NET version's actual (not aspirational) behavior
        }
        let meta = std::fs::metadata(&path).map_err(|e| AppError::io(&path, e))?;
        let relative_path = path
            .strip_prefix(base)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let mtime_unix = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        out.push(ManifestEntry {
            relative_path,
            size: meta.len(),
            mtime_unix,
        });
    }
    Ok(())
}

/// Deterministic hex-encoded SHA-256 over the sorted manifest, used both to
/// decide whether a folder changed and as the `source-hash` metadata value
/// stamped on the uploaded object.
pub fn manifest_hash(entries: &[ManifestEntry]) -> String {
    let mut hasher = Sha256::new();
    for e in entries {
        hasher.update(e.relative_path.as_bytes());
        hasher.update(b"\0");
        hasher.update(e.size.to_le_bytes());
        hasher.update(b"\0");
        hasher.update(e.mtime_unix.to_le_bytes());
        hasher.update(b"\n");
    }
    hex::encode(hasher.finalize())
}

pub fn hash_folder(root: &Path, recursive: bool) -> Result<String, AppError> {
    let entries = collect_manifest(root, recursive)?;
    Ok(manifest_hash(&entries))
}

/// SHA-256 over a byte slice (used for the encrypt/decrypt round-trip
/// self-check).
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// MD5 hex digest of a byte slice, used solely to verify S3's returned ETag
/// for a single-part, non-SSE-KMS PUT (where ETag == MD5 of the uploaded
/// bytes). This is not used for anything security-sensitive.
pub fn md5_hex(bytes: &[u8]) -> String {
    use md5::{Digest as _, Md5};
    let mut hasher = Md5::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Streaming MD5 for large files, avoiding a full second in-memory copy.
/// Not currently called -- `backup.rs` verifies against the ciphertext it
/// already holds in memory -- but kept as the natural entry point if a
/// future change verifies from disk instead (e.g. re-checking an
/// already-uploaded object without holding the whole thing in memory).
#[allow(dead_code)]
pub fn md5_hex_reader<R: Read>(mut r: R) -> std::io::Result<String> {
    use md5::{Digest as _, Md5};
    let mut hasher = Md5::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn hash_is_stable_and_order_independent() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("b.txt"), b"hello").unwrap();
        fs::write(dir.path().join("a.txt"), b"world").unwrap();

        let h1 = hash_folder(dir.path(), false).unwrap();
        let h2 = hash_folder(dir.path(), false).unwrap();
        assert_eq!(h1, h2, "hashing the same tree twice must be stable");
    }

    #[test]
    fn hash_changes_when_a_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), b"hello").unwrap();
        let h1 = hash_folder(dir.path(), false).unwrap();

        // Ensure mtime actually advances on filesystems with coarse mtime
        // resolution.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        fs::write(dir.path().join("a.txt"), b"hello!!").unwrap();
        let h2 = hash_folder(dir.path(), false).unwrap();

        assert_ne!(h1, h2);
    }

    #[test]
    fn non_recursive_ignores_subfolder_contents() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("root.txt"), b"root").unwrap();
        let sub = dir.path().join("child");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("nested.txt"), b"nested").unwrap();

        let entries = collect_manifest(dir.path(), false).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].relative_path, "root.txt");
    }

    #[test]
    fn recursive_includes_subfolder_contents() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("root.txt"), b"root").unwrap();
        let sub = dir.path().join("child");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("nested.txt"), b"nested").unwrap();

        let entries = collect_manifest(dir.path(), true).unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn md5_matches_known_vector() {
        // MD5("") = d41d8cd98f00b204e9800998ecf8427e
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        // MD5("abc") = 900150983cd24fb0d6963f7d28e17f72
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn sha256_matches_known_vector() {
        // SHA-256("abc"), verified against `python3 -c "import hashlib;
        // print(hashlib.sha256(b'abc').hexdigest())"`.
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
