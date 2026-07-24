use std::path::PathBuf;
use thiserror::Error;

pub const USAGE: &str = "s3b -action backup  -folder <backup_folder> [-bucket <s3_bucket>] [-force]\ns3b -action restore [-bucket <s3_bucket>] [-key <private_key_file>] [-object <object>]\ns3b -action test    [-bucket <s3_bucket>] [-key <private_key_file>]\ns3b -action genkey  [-out <key_prefix>]\n\nbackup   reads the recipient's public key path from S3BPUBKEY (or S3B-PUBKEY),\n         falling back to ~/.s3b/s3b.pub if neither is set\nrestore  falls back to ~/.s3b/s3b.key if -key is omitted\ntest     downloads, decrypts, and decompresses every object in the bucket to\n         verify the restore pipeline works end to end, deleting each result\n         immediately after -- nothing is left in place; -key falls back to\n         ~/.s3b/s3b.key if omitted, same as restore\n-bucket  falls back to BUCKET=<name> in ~/.s3b/s3b.aws if omitted\nAWS_ACCESS_KEY_ID/AWS_SECRET_ACCESS_KEY/AWS_REGION (env vars) fall back to\n         the matching key=value line in ~/.s3b/s3b.aws when not set in the\n         environment\n-force   re-uploads every folder, bypassing the content-hash change check\ngenkey   writes <key_prefix>.pub and <key_prefix>.key (prefix defaults to\n         ~/.s3b/s3b, creating ~/.s3b if needed)";

/// Top-level application error. `Usage` errors print the usage string and
/// exit 1; every other variant is logged and also exits 1 -- there is no
/// "succeed anyway" path, per the fail-closed requirement in the migration
/// notes.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("usage error: {0}")]
    Usage(String),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("s3 error: {0}")]
    S3(String),

    #[error("s3 object not found")]
    S3NotFound,

    #[error("encryption error: {0}")]
    Crypto(String),

    #[error("archive error: {0}")]
    Archive(String),

    #[error("backup completed with {0} folder(s) still failing after retries")]
    BackupIncomplete(usize),

    #[error("restore completed with {0} object(s) failing")]
    RestoreIncomplete(usize),

    #[error("test completed with {0} object(s) failing")]
    TestIncomplete(usize),
}

impl AppError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        AppError::Io {
            path: path.into(),
            source,
        }
    }
}

/// Convenience alias, kept as part of this module's public surface for any
/// future function that wants to name the result type explicitly.
#[allow(dead_code)]
pub type AppResult<T> = Result<T, AppError>;
