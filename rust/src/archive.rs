//! In-process archive + compress, replacing the original three-stage
//! tar/gzip/openssl external-process pipeline.
//!
//! `tar::Builder` writes directly into a `flate2::GzEncoder` which writes
//! directly into the destination file, so folder contents are streamed
//! straight to a gzip'd tarball on disk without ever materializing an
//! intermediate `.tar` file, and without buffering the whole folder in
//! memory. The destination is written to a `.tmp` path and renamed into
//! place atomically once complete, so a crash mid-archive can never leave a
//! half-written file where a caller expects a complete one.

use flate2::write::GzEncoder;
use flate2::Compression;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use tar::{Archive, Builder};

use crate::error::AppError;

/// Archives `source` into a gzip'd tarball at `dest_tar_gz`.
///
/// - `recursive = true`: the whole subtree under `source` is archived
///   (used for the backup root's immediate child folders).
/// - `recursive = false`: only the regular files directly inside `source`
///   are archived, each as a top-level tar entry with just its file name
///   (used for the backup root itself, so subfolder contents already
///   covered by their own archives aren't duplicated).
pub fn create_tar_gz(source: &Path, recursive: bool, dest_tar_gz: &Path) -> Result<(), AppError> {
    let tmp_path = tmp_path_for(dest_tar_gz);

    {
        let file = File::create(&tmp_path).map_err(|e| AppError::io(&tmp_path, e))?;
        let writer = BufWriter::new(file);
        let encoder = GzEncoder::new(writer, Compression::default());
        let mut builder = Builder::new(encoder);
        // Deterministic-ish ordering (and testability) even though we
        // don't strictly need reproducible archives.
        builder.mode(tar::HeaderMode::Deterministic);

        if recursive {
            builder
                .append_dir_all(".", source)
                .map_err(|e| AppError::Archive(format!("archiving {}: {e}", source.display())))?;
        } else {
            let mut names: Vec<_> = std::fs::read_dir(source)
                .map_err(|e| AppError::io(source, e))?
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
                .map(|e| e.path())
                .collect();
            names.sort();
            for path in names {
                let file_name = path
                    .file_name()
                    .ok_or_else(|| AppError::Archive(format!("no file name for {}", path.display())))?;
                builder
                    .append_path_with_name(&path, file_name)
                    .map_err(|e| AppError::Archive(format!("archiving {}: {e}", path.display())))?;
            }
        }

        let encoder = builder
            .into_inner()
            .map_err(|e| AppError::Archive(format!("finalizing tar stream: {e}")))?;
        encoder
            .finish()
            .map_err(|e| AppError::Archive(format!("finalizing gzip stream: {e}")))?;
    }

    std::fs::rename(&tmp_path, dest_tar_gz).map_err(|e| AppError::io(dest_tar_gz, e))?;
    Ok(())
}

/// Expands a gzip'd tarball into `dest_dir` (restore's "decompress" +
/// "expand" steps collapsed into one, since both are just reading the same
/// stream through two decoders).
pub fn expand_tar_gz(tar_gz_path: &Path, dest_dir: &Path) -> Result<(), AppError> {
    std::fs::create_dir_all(dest_dir).map_err(|e| AppError::io(dest_dir, e))?;
    let file = File::open(tar_gz_path).map_err(|e| AppError::io(tar_gz_path, e))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    archive
        .unpack(dest_dir)
        .map_err(|e| AppError::Archive(format!("expanding {}: {e}", tar_gz_path.display())))?;
    Ok(())
}

fn tmp_path_for(dest: &Path) -> std::path::PathBuf {
    let mut name = dest
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".tmp");
    dest.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn recursive_round_trip_preserves_files() {
        let src = tempfile::tempdir().unwrap();
        fs::write(src.path().join("a.txt"), b"file a").unwrap();
        let sub = src.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("b.txt"), b"file b").unwrap();

        let workdir = tempfile::tempdir().unwrap();
        let archive_path = workdir.path().join("out.tar.gz");
        create_tar_gz(src.path(), true, &archive_path).unwrap();
        assert!(archive_path.exists());
        assert!(!tmp_path_for(&archive_path).exists(), "tmp file should be renamed away");

        let dest = workdir.path().join("expanded");
        expand_tar_gz(&archive_path, &dest).unwrap();

        assert_eq!(fs::read(dest.join("a.txt")).unwrap(), b"file a");
        assert_eq!(fs::read(dest.join("sub").join("b.txt")).unwrap(), b"file b");
    }

    #[test]
    fn non_recursive_skips_subfolder_contents() {
        let src = tempfile::tempdir().unwrap();
        fs::write(src.path().join("root.txt"), b"root file").unwrap();
        let sub = src.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("nested.txt"), b"nested file").unwrap();

        let workdir = tempfile::tempdir().unwrap();
        let archive_path = workdir.path().join("out.tar.gz");
        create_tar_gz(src.path(), false, &archive_path).unwrap();

        let dest = workdir.path().join("expanded");
        expand_tar_gz(&archive_path, &dest).unwrap();

        assert_eq!(fs::read(dest.join("root.txt")).unwrap(), b"root file");
        assert!(!dest.join("sub").exists());
    }

    #[test]
    fn empty_folder_round_trips() {
        let src = tempfile::tempdir().unwrap();
        let workdir = tempfile::tempdir().unwrap();
        let archive_path = workdir.path().join("empty.tar.gz");
        create_tar_gz(src.path(), true, &archive_path).unwrap();
        let dest = workdir.path().join("expanded");
        expand_tar_gz(&archive_path, &dest).unwrap();
        assert!(dest.exists());
    }
}
