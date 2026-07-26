//! Typed configuration, replacing the original stringly-typed `Config`
//! dictionary + recursive `$(key)` template substitution.
//!
//! Everything that used to be a template string (temp dir, bucket-related
//! paths) is now just a field on this struct; the only remaining "variable"
//! content (bucket name, object name) is passed explicitly as function
//! arguments rather than woven into command templates, since there are no
//! more external commands to template into.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::crypto::DEFAULT_KEY_DIR;
use crate::error::AppError;

/// Filename (under `DEFAULT_KEY_DIR`, i.e. `~/.s3b`) of the fallback file
/// for AWS credentials/region and the default bucket -- `key=value` lines,
/// one per line, checked when the corresponding environment variable or
/// `-bucket` flag isn't set. Same "check env/CLI first, fall back to a file
/// under `~/.s3b`" pattern as `crypto::resolve_private_key_path`.
const AWS_CREDENTIALS_FILENAME: &str = "s3b.aws";

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FileConfig {
    /// Working/scratch directory for the one intermediate temp file per
    /// folder (tar+gzip stream, then in place replaced by the encrypted
    /// version before upload). Equivalent to the old `s3b.temp`.
    pub temp_dir: String,

    /// AWS region used to build the default `*.s3.<region>.amazonaws.com`
    /// endpoint. Ignored if `s3_endpoint` is set.
    pub region: String,

    /// Override endpoint for S3-compatible providers other than AWS
    /// (e.g. `https://s3.us-west-000.backblazeb2.com`). When set, path-style
    /// addressing (`<endpoint>/<bucket>/<key>`) is used instead of
    /// virtual-hosted-style.
    pub s3_endpoint: Option<String>,

    /// How many additional attempts to make for a folder/object that fails
    /// upload verification or download, beyond the first (i.e. 3 here means
    /// 4 total attempts) -- mirrors the original "up to 3 retries" behavior.
    pub retry_attempts: u32,

    /// Override for the hostname embedded in object names / metadata.
    /// Defaults to the OS-reported hostname.
    pub hostname: Option<String>,

    /// Override for the username embedded in object names / metadata.
    /// Defaults to `$USER`/`$USERNAME`.
    pub username: Option<String>,

    /// Size threshold (bytes) above which an upload switches from a single
    /// PUT to S3 multipart upload, split into `multipart_part_size_bytes`
    /// parts. Defaults to the AWS CLI's own multipart threshold, so
    /// small-file behavior doesn't change from a plain PUT.
    pub multipart_threshold_bytes: u64,

    /// Size (bytes) of each part when a multipart upload is used. S3
    /// requires at least 5 MiB for every part but the last and allows at
    /// most 10,000 parts per object; smaller parts mean more, smaller,
    /// independently-retriable requests (useful on flaky networks) at the
    /// cost of more round-trips.
    pub multipart_part_size_bytes: u64,

    /// How many additional attempts each individual network call in a
    /// multipart upload gets beyond the first (each part upload, plus the
    /// create/complete calls bookending them), with exponential backoff
    /// between attempts. Kept separate from `retry_attempts` (which retries
    /// a whole folder -- a full re-archive/re-encrypt/re-upload) since a
    /// single part is cheap to retry and a large upload makes enough
    /// requests that some of them failing transiently is expected, not
    /// exceptional.
    pub multipart_part_retry_attempts: u32,
}

impl Default for FileConfig {
    fn default() -> Self {
        // std::env::temp_dir() resolves to the right place per platform
        // ($TMPDIR/tmp on Unix, %TEMP% on Windows) rather than hardcoding a
        // Unix path that doesn't exist on Windows.
        let temp_dir = std::env::temp_dir()
            .join("s3b")
            .to_string_lossy()
            .to_string();

        FileConfig {
            temp_dir,
            region: "us-east-1".to_string(),
            s3_endpoint: None,
            retry_attempts: 3,
            hostname: None,
            username: None,
            multipart_threshold_bytes: 8 * 1024 * 1024,
            multipart_part_size_bytes: 8 * 1024 * 1024,
            multipart_part_retry_attempts: 5,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub temp_dir: PathBuf,
    pub region: String,
    pub s3_endpoint: Option<String>,
    pub retry_attempts: u32,
    pub hostname: String,
    pub username: String,
    pub aws_access_key_id: String,
    pub aws_secret_access_key: String,
    pub aws_session_token: Option<String>,
    /// `BUCKET` from `~/.s3b/s3b.aws`, if present. Used as the fallback
    /// target bucket when `-bucket` isn't given -- see `resolve_bucket`.
    pub bucket: Option<String>,
    pub multipart_threshold_bytes: u64,
    pub multipart_part_size_bytes: u64,
    pub multipart_part_retry_attempts: u32,
}

fn env_first(names: &[&str]) -> Option<String> {
    for n in names {
        if let Ok(v) = std::env::var(n) {
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

/// Parses `s3b.aws`-style content: `key=value` pairs, one per line, with
/// blank lines and lines starting with `#` ignored. Whitespace around the
/// key and value is trimmed, so `KEY = value` and `KEY=value` both work.
fn parse_aws_file(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    map
}

/// Reads and parses the AWS credentials/bucket fallback file at `path`, or
/// returns an empty map if it doesn't exist (not finding the file is not an
/// error -- it's an optional fallback, checked only after the environment
/// and CLI flags come up empty).
fn load_aws_file_at(path: &Path) -> HashMap<String, String> {
    std::fs::read_to_string(path)
        .map(|text| parse_aws_file(&text))
        .unwrap_or_default()
}

/// `~/.s3b/s3b.aws` under the given home directory.
fn aws_file_path_from_home(home: &Path) -> PathBuf {
    home.join(DEFAULT_KEY_DIR).join(AWS_CREDENTIALS_FILENAME)
}

/// `~/.s3b/s3b.aws` (`%USERPROFILE%\.s3b\s3b.aws` on Windows), or `None` if
/// neither `HOME` nor `USERPROFILE` is set. Mirrors
/// `crypto::default_key_dir`'s env-var fallback order.
fn aws_file_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| aws_file_path_from_home(Path::new(&home)))
}

/// Loads `~/.s3b/s3b.aws` (see `aws_file_path`), or an empty map if it
/// doesn't exist or no home directory can be resolved.
fn load_aws_file() -> HashMap<String, String> {
    aws_file_path()
        .map(|p| load_aws_file_at(&p))
        .unwrap_or_default()
}

/// Looks up the first of `names` present in `map`, mirroring `env_first`'s
/// "first match wins" order -- used to accept more than one key name for
/// the same setting (e.g. `AWS_ACCESS_KEY_ID` and the shorter
/// `AWS_ACCESS_KEY`).
fn file_lookup(map: &HashMap<String, String>, names: &[&str]) -> Option<String> {
    names.iter().find_map(|n| map.get(*n)).cloned()
}

impl Config {
    /// Loads `<config_path>` (or defaults if absent), then resolves
    /// everything that must come from the environment: AWS credentials.
    /// When `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`/`AWS_REGION` aren't
    /// set in the environment, falls back to the matching `key=value` line
    /// in `~/.s3b/s3b.aws` (`AWS_ACCESS_KEY` is also accepted there as a
    /// shorter alias for `AWS_ACCESS_KEY_ID`); the same file's `BUCKET` line
    /// is read into `Config::bucket` as the `-bucket` fallback (see
    /// `resolve_bucket`). Fails fast (before any pipeline work starts) if
    /// a required value is missing from every source, matching the original
    /// "fail before any work starts" behavior. The recipient public/private
    /// key paths are *not* resolved here -- `genkey` needs neither, `backup`
    /// resolves the public key itself via `crypto::resolve_and_load_public_key`,
    /// and `restore` resolves the private key path via
    /// `crypto::resolve_private_key_path` -- so a bare `Config` no longer
    /// implies "a key secret is available."
    pub fn load(config_path: Option<&str>) -> Result<Config, AppError> {
        let file_cfg: FileConfig = match config_path {
            Some(p) => {
                let text = std::fs::read_to_string(p).map_err(|e| AppError::io(p, e))?;
                toml::from_str(&text)
                    .map_err(|e| AppError::Config(format!("invalid config file {p}: {e}")))?
            }
            None => {
                // Optional convention: ./s3b.toml if present, else defaults.
                if Path::new("s3b.toml").exists() {
                    let text = std::fs::read_to_string("s3b.toml")
                        .map_err(|e| AppError::io("s3b.toml", e))?;
                    toml::from_str(&text)
                        .map_err(|e| AppError::Config(format!("invalid s3b.toml: {e}")))?
                } else {
                    FileConfig::default()
                }
            }
        };

        let aws_file = load_aws_file();

        let aws_access_key_id = env_first(&["AWS_ACCESS_KEY_ID"])
            .or_else(|| file_lookup(&aws_file, &["AWS_ACCESS_KEY_ID", "AWS_ACCESS_KEY"]))
            .ok_or_else(|| {
                AppError::Config(
                    "AWS_ACCESS_KEY_ID is not set (checked the environment and ~/.s3b/s3b.aws)"
                        .into(),
                )
            })?;
        let aws_secret_access_key = env_first(&["AWS_SECRET_ACCESS_KEY"])
            .or_else(|| file_lookup(&aws_file, &["AWS_SECRET_ACCESS_KEY"]))
            .ok_or_else(|| {
                AppError::Config(
                    "AWS_SECRET_ACCESS_KEY is not set (checked the environment and ~/.s3b/s3b.aws)"
                        .into(),
                )
            })?;
        let aws_session_token = env_first(&["AWS_SESSION_TOKEN"]);

        let hostname = file_cfg
            .hostname
            .clone()
            .or_else(|| env_first(&["HOSTNAME", "COMPUTERNAME"]))
            .or_else(hostname_fallback)
            .unwrap_or_else(|| "unknown-host".to_string());

        let username = file_cfg
            .username
            .clone()
            .or_else(|| env_first(&["USER", "USERNAME"]))
            .unwrap_or_else(|| "unknown-user".to_string());

        let region = env_first(&["AWS_REGION", "AWS_DEFAULT_REGION"])
            .or_else(|| file_lookup(&aws_file, &["AWS_REGION"]))
            .unwrap_or(file_cfg.region);

        let bucket = file_lookup(&aws_file, &["BUCKET"]);

        Ok(Config {
            temp_dir: PathBuf::from(file_cfg.temp_dir),
            region,
            s3_endpoint: file_cfg.s3_endpoint,
            retry_attempts: file_cfg.retry_attempts,
            hostname,
            username,
            aws_access_key_id,
            aws_secret_access_key,
            aws_session_token,
            bucket,
            multipart_threshold_bytes: file_cfg.multipart_threshold_bytes,
            multipart_part_size_bytes: file_cfg.multipart_part_size_bytes,
            multipart_part_retry_attempts: file_cfg.multipart_part_retry_attempts,
        })
    }

    /// Resolves the target bucket: `explicit` (the `-bucket` CLI flag) if
    /// given, else `self.bucket` (the `BUCKET` line from `~/.s3b/s3b.aws`,
    /// loaded by `Config::load`). Mirrors
    /// `crypto::resolve_private_key_path`'s explicit-flag-then-file-fallback
    /// shape.
    pub fn resolve_bucket(&self, explicit: Option<&str>) -> Result<String, AppError> {
        resolve_bucket_from(explicit, self.bucket.as_deref())
    }

    /// The `restore/` subfolder is the one exception to `reset_temp_dir`'s
    /// wipe: `-action restore`'s finished, expanded output for each object
    /// lives here rather than directly under `temp_dir`, specifically so it
    /// survives across runs. That's what lets a rerun after a partial
    /// restore failure (a dropped connection, a DNS blip) skip objects that
    /// already finished instead of re-downloading and re-expanding
    /// everything from scratch -- see `restore::restore_one`.
    pub const RESTORE_SUBDIR: &'static str = "restore";

    /// `<temp_dir>/restore` -- see `RESTORE_SUBDIR`.
    pub fn restore_dir(&self) -> PathBuf {
        self.temp_dir.join(Self::RESTORE_SUBDIR)
    }

    /// Wipes everything directly under the temp directory *except* the
    /// `restore/` subfolder, then ensures both it and `restore/` exist.
    /// Called once at startup, before any backup/restore/test work begins.
    ///
    /// `temp_dir` is otherwise pure scratch space: intermediate archive/
    /// compress/encrypt files during backup, and the downloaded/decrypted
    /// tarball plus test's expanded output during restore/test. There's no
    /// resumable mid-pipeline state for any of that (per the migration
    /// notes), so nothing outside `restore/` is meant to survive between
    /// runs -- but a killed or crashed previous run can still leave partial
    /// `.tmp`/`.enc` files or expanded folders behind, and starting the next
    /// run on top of that residue risks confusing output rather than a
    /// clean rerun. Always starting from an empty scratch area removes that
    /// risk entirely, without touching the one thing (`restore/`) that's
    /// deliberately meant to persist.
    pub fn reset_temp_dir(&self) -> Result<(), AppError> {
        match std::fs::read_dir(&self.temp_dir) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry.map_err(|e| AppError::io(&self.temp_dir, e))?;
                    if entry.file_name() == Self::RESTORE_SUBDIR {
                        continue;
                    }
                    let path = entry.path();
                    let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    let remove_result = if is_dir {
                        std::fs::remove_dir_all(&path)
                    } else {
                        std::fs::remove_file(&path)
                    };
                    if let Err(e) = remove_result {
                        if e.kind() != std::io::ErrorKind::NotFound {
                            return Err(AppError::io(&path, e));
                        }
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(AppError::io(&self.temp_dir, e)),
        }
        std::fs::create_dir_all(&self.temp_dir).map_err(|e| AppError::io(&self.temp_dir, e))?;
        let restore_dir = self.restore_dir();
        std::fs::create_dir_all(&restore_dir).map_err(|e| AppError::io(&restore_dir, e))
    }
}

fn resolve_bucket_from(explicit: Option<&str>, file_bucket: Option<&str>) -> Result<String, AppError> {
    explicit.or(file_bucket).map(str::to_string).ok_or_else(|| {
        AppError::Config(
            "no bucket configured: pass -bucket <name>, or set BUCKET=<name> in ~/.s3b/s3b.aws"
                .into(),
        )
    })
}

/// Best-effort hostname lookup for when neither the config file nor
/// `$HOSTNAME`/`$COMPUTERNAME` supplied one.
///
/// `/etc/hostname` (tried first) covers most Linux distros, where it's
/// populated by the OS. It does *not* cover macOS: macOS stores the
/// hostname via `scutil`/SystemConfiguration instead and never writes
/// `/etc/hostname`, and interactive shells (zsh in particular) generally
/// don't export `$HOSTNAME` to child processes even though it's set as a
/// shell parameter. The `hostname` command is present on both platforms and
/// is what resolves it correctly there.
fn hostname_fallback() -> Option<String> {
    if let Some(h) = std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        return Some(h);
    }

    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_file_config_matches_docs() {
        let d = FileConfig::default();
        // Platform-dependent (std::env::temp_dir()/s3b): just check it ends
        // up under the OS temp dir with an "s3b" leaf, rather than asserting
        // a Unix-only literal path.
        assert!(d.temp_dir.ends_with("s3b"));
        assert_eq!(d.retry_attempts, 3);
        assert_eq!(d.multipart_threshold_bytes, 8 * 1024 * 1024);
        assert_eq!(d.multipart_part_size_bytes, 8 * 1024 * 1024);
        assert_eq!(d.multipart_part_retry_attempts, 5);
    }

    #[test]
    fn env_first_picks_first_present() {
        std::env::remove_var("S3B_TEST_A");
        std::env::set_var("S3B_TEST_B", "value-b");
        assert_eq!(
            env_first(&["S3B_TEST_A", "S3B_TEST_B"]),
            Some("value-b".to_string())
        );
        std::env::remove_var("S3B_TEST_B");
    }

    #[test]
    fn parse_aws_file_reads_key_value_lines_ignoring_blanks_and_comments() {
        let text = "AWS_ACCESS_KEY_ID=AKIA123\n\
             # a comment line\n\
             \n\
             AWS_SECRET_ACCESS_KEY = super-secret\n\
             BUCKET=my-bucket\n\
             AWS_REGION=us-west-2\n";
        let map = parse_aws_file(text);
        assert_eq!(map.get("AWS_ACCESS_KEY_ID").map(String::as_str), Some("AKIA123"));
        // Whitespace around '=' is trimmed on both sides.
        assert_eq!(
            map.get("AWS_SECRET_ACCESS_KEY").map(String::as_str),
            Some("super-secret")
        );
        assert_eq!(map.get("BUCKET").map(String::as_str), Some("my-bucket"));
        assert_eq!(map.get("AWS_REGION").map(String::as_str), Some("us-west-2"));
        assert_eq!(map.len(), 4, "blank line and comment must not produce entries");
    }

    #[test]
    fn file_lookup_checks_names_in_order() {
        let mut map = HashMap::new();
        map.insert("AWS_ACCESS_KEY".to_string(), "alias-value".to_string());
        assert_eq!(
            file_lookup(&map, &["AWS_ACCESS_KEY_ID", "AWS_ACCESS_KEY"]).as_deref(),
            Some("alias-value")
        );

        map.insert("AWS_ACCESS_KEY_ID".to_string(), "canonical-value".to_string());
        assert_eq!(
            file_lookup(&map, &["AWS_ACCESS_KEY_ID", "AWS_ACCESS_KEY"]).as_deref(),
            Some("canonical-value"),
            "earlier name in the list wins when both are present"
        );

        assert_eq!(file_lookup(&map, &["NOT_PRESENT"]), None);
    }

    #[test]
    fn aws_file_path_from_home_is_dot_s3b_s3b_aws() {
        let p = aws_file_path_from_home(Path::new("/home/eric"));
        assert_eq!(p, PathBuf::from("/home/eric/.s3b/s3b.aws"));
    }

    #[test]
    fn load_aws_file_at_parses_existing_file_and_defaults_missing_file_to_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s3b.aws");
        std::fs::write(&path, "BUCKET=from-file\n").unwrap();

        let map = load_aws_file_at(&path);
        assert_eq!(map.get("BUCKET").map(String::as_str), Some("from-file"));

        let missing = load_aws_file_at(&dir.path().join("does-not-exist"));
        assert!(missing.is_empty());
    }

    fn test_config(temp_dir: PathBuf) -> Config {
        Config {
            temp_dir,
            region: "us-east-1".to_string(),
            s3_endpoint: None,
            retry_attempts: 3,
            hostname: "host".to_string(),
            username: "user".to_string(),
            aws_access_key_id: "AKIA".to_string(),
            aws_secret_access_key: "secret".to_string(),
            aws_session_token: None,
            bucket: None,
            multipart_threshold_bytes: 8 * 1024 * 1024,
            multipart_part_size_bytes: 8 * 1024 * 1024,
            multipart_part_retry_attempts: 5,
        }
    }

    #[test]
    fn reset_temp_dir_wipes_existing_content_and_recreates_empty() {
        let base = tempfile::tempdir().unwrap();
        let temp_dir = base.path().join("s3b-temp");
        std::fs::create_dir_all(temp_dir.join("leftover")).unwrap();
        std::fs::write(temp_dir.join("leftover").join("stray.tmp"), b"x").unwrap();
        std::fs::write(temp_dir.join("another.tmp"), b"y").unwrap();

        let cfg = test_config(temp_dir.clone());
        cfg.reset_temp_dir().unwrap();

        assert!(temp_dir.is_dir());
        assert_eq!(std::fs::read_dir(&temp_dir).unwrap().count(), 0);
    }

    #[test]
    fn reset_temp_dir_creates_dir_if_it_does_not_exist_yet() {
        let base = tempfile::tempdir().unwrap();
        let temp_dir = base.path().join("does-not-exist-yet");

        let cfg = test_config(temp_dir.clone());
        cfg.reset_temp_dir().unwrap();

        assert!(temp_dir.is_dir());
        assert!(cfg.restore_dir().is_dir(), "restore/ should be created too");
    }

    #[test]
    fn reset_temp_dir_preserves_restore_subfolder_but_wipes_everything_else() {
        let base = tempfile::tempdir().unwrap();
        let temp_dir = base.path().join("s3b-temp");
        let cfg = test_config(temp_dir.clone());

        // Prior restore output that must survive.
        let restored_object_dir = cfg.restore_dir().join("host_user_Documents");
        std::fs::create_dir_all(&restored_object_dir).unwrap();
        std::fs::write(restored_object_dir.join("file.txt"), b"restored content").unwrap();
        std::fs::write(cfg.restore_dir().join(".s3b-restore-complete"), b"deadbeef").unwrap();

        // Ordinary scratch residue that must not survive.
        std::fs::write(temp_dir.join("some.tar.gz.tmp"), b"scratch").unwrap();

        cfg.reset_temp_dir().unwrap();

        assert_eq!(
            std::fs::read(restored_object_dir.join("file.txt")).unwrap(),
            b"restored content",
            "previously restored output must survive reset_temp_dir"
        );
        assert!(!temp_dir.join("some.tar.gz.tmp").exists(), "non-restore scratch files must still be wiped");
    }

    #[test]
    fn resolve_bucket_prefers_explicit_then_file_then_errors() {
        assert_eq!(
            resolve_bucket_from(Some("cli-bucket"), Some("file-bucket")).unwrap(),
            "cli-bucket"
        );
        assert_eq!(
            resolve_bucket_from(None, Some("file-bucket")).unwrap(),
            "file-bucket"
        );
        assert!(matches!(
            resolve_bucket_from(None, None).unwrap_err(),
            AppError::Config(_)
        ));
    }
}
