//! Typed configuration, replacing the original stringly-typed `Config`
//! dictionary + recursive `$(key)` template substitution.
//!
//! Everything that used to be a template string (temp dir, bucket-related
//! paths) is now just a field on this struct; the only remaining "variable"
//! content (bucket name, object name) is passed explicitly as function
//! arguments rather than woven into command templates, since there are no
//! more external commands to template into.

use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::error::AppError;

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

impl Config {
    /// Loads `<config_path>` (or defaults if absent), then resolves
    /// everything that must come from the environment: AWS credentials.
    /// Fails fast (before any pipeline work starts) if any required value is
    /// missing, matching the original "fail before any work starts"
    /// behavior. The recipient public/private key paths are *not* resolved
    /// here -- `genkey` needs neither, `backup` resolves the public key
    /// itself via `crypto::resolve_and_load_public_key`, and `restore`
    /// receives the private key path directly from the `-key` CLI flag --
    /// so a bare `Config` no longer implies "a key secret is available."
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

        let aws_access_key_id = env_first(&["AWS_ACCESS_KEY_ID"])
            .ok_or_else(|| AppError::Config("AWS_ACCESS_KEY_ID is not set".into()))?;
        let aws_secret_access_key = env_first(&["AWS_SECRET_ACCESS_KEY"])
            .ok_or_else(|| AppError::Config("AWS_SECRET_ACCESS_KEY is not set".into()))?;
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

        let region = env_first(&["AWS_REGION", "AWS_DEFAULT_REGION"]).unwrap_or(file_cfg.region);

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
        })
    }
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
}
