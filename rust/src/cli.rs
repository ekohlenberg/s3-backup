//! Hand-rolled `-key value` argument parsing, mirroring the original scheme
//! rather than pulling in `clap` for a small, fixed set of verbs (see
//! migration notes).

use crate::error::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Backup,
    Restore,
    Genkey,
}

#[derive(Debug, Clone)]
pub struct Args {
    pub action: Action,
    /// Optional for backup/restore: target S3 bucket. Falls back to
    /// `BUCKET=<name>` in `~/.s3b/s3b.aws` (`config::Config::resolve_bucket`)
    /// when omitted; unused for genkey.
    pub bucket: Option<String>,
    /// Required for backup.
    pub folder: Option<String>,
    /// Optional for restore; unused elsewhere.
    pub object: Option<String>,
    pub config_path: Option<String>,
    /// Required for genkey: the `<prefix>` written to `<prefix>.pub`/`<prefix>.key`.
    pub out: Option<String>,
    /// Optional for restore: path to the private key file. Falls back to
    /// `~/.s3b/s3b.key` (`crypto::resolve_private_key_path`) when omitted.
    pub key: Option<String>,
    /// `-force`, backup only: upload every folder regardless of the
    /// content-hash change check.
    pub force: bool,
}

/// Flags that stand alone (no following value) -- everything else follows
/// the strict `-key value` pairing described below.
const BOOLEAN_FLAGS: &[&str] = &["force"];

/// Parses `argv` (excluding the program name) into a flat `-key value` map,
/// then validates it into `Args`.
///
/// Requirements enforced here (from the requirements doc, extended for the
/// `genkey` action added by the recipient-keypair encryption change):
/// - any `-flag` with no following non-dash token is invalid, *except* the
///   flags listed in `BOOLEAN_FLAGS`, which take no value at all
/// - `-action` is required and must be exactly `backup`, `restore`, or `genkey`
/// - `backup` requires `-folder`; `-bucket` and `-force` are optional --
///   `-bucket` falls back to `BUCKET=<name>` in `~/.s3b/s3b.aws`
///   (`config::Config::resolve_bucket`)
/// - `restore` has no required flags -- `-bucket`, `-key`, and `-object` are
///   all optional; `-bucket` falls back the same way as for `backup`, and
///   `-key` falls back to `~/.s3b/s3b.key` (`crypto::resolve_private_key_path`)
/// - `genkey`'s `-out` is optional (defaults to `~/.s3b/s3b`,
///   `crypto::DEFAULT_KEY_PREFIX` under `crypto::DEFAULT_KEY_DIR`)
pub fn parse(argv: &[String]) -> Result<Args, AppError> {
    let mut map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut force = false;

    let mut i = 0;
    while i < argv.len() {
        let tok = &argv[i];
        if let Some(key) = tok.strip_prefix('-') {
            if key.is_empty() {
                return Err(AppError::Usage(format!("empty flag at position {i}")));
            }
            if BOOLEAN_FLAGS.contains(&key) {
                if key == "force" {
                    force = true;
                }
                i += 1;
                continue;
            }
            let next = argv.get(i + 1);
            match next {
                Some(v) if !v.starts_with('-') => {
                    map.insert(key.to_string(), v.clone());
                    i += 2;
                }
                _ => {
                    return Err(AppError::Usage(format!("flag -{key} requires a value")));
                }
            }
        } else {
            return Err(AppError::Usage(format!(
                "unexpected token '{tok}' (expected a -flag)"
            )));
        }
    }

    let action_str = map
        .remove("action")
        .ok_or_else(|| AppError::Usage("-action is required".into()))?;
    let action = match action_str.as_str() {
        "backup" => Action::Backup,
        "restore" => Action::Restore,
        "genkey" => Action::Genkey,
        other => {
            return Err(AppError::Usage(format!(
                "-action must be 'backup', 'restore', or 'genkey', got '{other}'"
            )))
        }
    };

    let bucket = map.remove("bucket");
    let folder = map.remove("folder");
    let object = map.remove("object");
    let config_path = map.remove("config");
    let out = map.remove("out");
    let key = map.remove("key");

    match action {
        Action::Backup => {
            if folder.is_none() {
                return Err(AppError::Usage("-folder is required for -action backup".into()));
            }
            // -bucket is optional here; if omitted, main.rs resolves it via
            // Config::resolve_bucket (falls back to BUCKET=<name> in
            // ~/.s3b/s3b.aws), which reports a Config error of its own if
            // that can't be resolved either.
        }
        Action::Restore => {
            // -bucket is optional here (see the Backup arm above); -key is
            // optional too -- if omitted, main.rs resolves it via
            // crypto::resolve_private_key_path (falls back to
            // ~/.s3b/s3b.key), which reports a Config error of its own if
            // that can't be resolved either.
        }
        Action::Genkey => {
            // -out is optional here; the default prefix is applied at the
            // point of use (main.rs), not during parsing/validation.
        }
    }

    Ok(Args {
        action,
        bucket,
        folder,
        object,
        config_path,
        out,
        key,
        force,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn backup_requires_folder() {
        let err = parse(&v(&["-action", "backup", "-bucket", "b"])).unwrap_err();
        assert!(matches!(err, AppError::Usage(_)));
    }

    #[test]
    fn backup_bucket_optional_at_parse_time() {
        // -bucket is no longer required by cli::parse itself -- main.rs
        // falls back to BUCKET=<name> in ~/.s3b/s3b.aws
        // (config::Config::resolve_bucket) when it's omitted, and reports
        // its own error if that's not set either.
        let a = parse(&v(&["-action", "backup", "-folder", "/tmp/x"])).unwrap();
        assert_eq!(a.action, Action::Backup);
        assert_eq!(a.bucket, None);
    }

    #[test]
    fn backup_ok() {
        let a = parse(&v(&["-action", "backup", "-folder", "/tmp/x", "-bucket", "b"])).unwrap();
        assert_eq!(a.action, Action::Backup);
        assert_eq!(a.folder.as_deref(), Some("/tmp/x"));
        assert_eq!(a.bucket.as_deref(), Some("b"));
        assert!(!a.force, "force should default to false");
    }

    #[test]
    fn backup_force_flag() {
        let a = parse(&v(&[
            "-action", "backup", "-folder", "/tmp/x", "-bucket", "b", "-force",
        ]))
        .unwrap();
        assert!(a.force);
    }

    #[test]
    fn force_flag_takes_no_value_and_does_not_consume_the_next_flag() {
        // -force appearing before another -flag must not swallow it as a value.
        let a = parse(&v(&[
            "-action", "backup", "-force", "-folder", "/tmp/x", "-bucket", "b",
        ]))
        .unwrap();
        assert!(a.force);
        assert_eq!(a.folder.as_deref(), Some("/tmp/x"));
        assert_eq!(a.bucket.as_deref(), Some("b"));
    }

    #[test]
    fn force_flag_alone_at_end_of_argv_does_not_error() {
        // Previously (before -force was a boolean flag) a trailing -flag
        // with nothing after it was a usage error ("requires a value").
        let a = parse(&v(&["-action", "genkey", "-force"])).unwrap();
        assert!(a.force);
    }

    #[test]
    fn restore_key_optional_at_parse_time() {
        // -key is no longer required by cli::parse itself -- main.rs resolves
        // a default (~/.s3b/s3b.key) via crypto::resolve_private_key_path
        // when it's omitted, and reports its own error if that fails too.
        let a = parse(&v(&["-action", "restore", "-bucket", "b"])).unwrap();
        assert_eq!(a.action, Action::Restore);
        assert_eq!(a.key, None);
    }

    #[test]
    fn restore_object_optional() {
        let a = parse(&v(&[
            "-action", "restore", "-bucket", "b", "-key", "/tmp/priv.key",
        ]))
        .unwrap();
        assert_eq!(a.action, Action::Restore);
        assert_eq!(a.object, None);
        assert_eq!(a.key.as_deref(), Some("/tmp/priv.key"));
    }

    #[test]
    fn restore_with_object() {
        let a = parse(&v(&[
            "-action", "restore", "-bucket", "b", "-key", "/tmp/priv.key", "-object",
            "host_user_path.tar.gz.enc",
        ]))
        .unwrap();
        assert_eq!(a.object.as_deref(), Some("host_user_path.tar.gz.enc"));
    }

    #[test]
    fn genkey_out_is_optional() {
        let a = parse(&v(&["-action", "genkey"])).unwrap();
        assert_eq!(a.action, Action::Genkey);
        assert_eq!(a.out, None);
    }

    #[test]
    fn genkey_ok() {
        let a = parse(&v(&["-action", "genkey", "-out", "/tmp/s3b"])).unwrap();
        assert_eq!(a.action, Action::Genkey);
        assert_eq!(a.out.as_deref(), Some("/tmp/s3b"));
        assert_eq!(a.bucket, None);
    }

    #[test]
    fn invalid_action_rejected() {
        let err = parse(&v(&["-action", "delete", "-bucket", "b"])).unwrap_err();
        assert!(matches!(err, AppError::Usage(_)));
    }

    #[test]
    fn dangling_flag_rejected() {
        let err = parse(&v(&["-action", "backup", "-folder"])).unwrap_err();
        assert!(matches!(err, AppError::Usage(_)));
    }

    #[test]
    fn missing_action_rejected() {
        let err = parse(&v(&["-bucket", "b"])).unwrap_err();
        assert!(matches!(err, AppError::Usage(_)));
    }
}
