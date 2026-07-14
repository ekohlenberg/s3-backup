//! Hand-rolled `-key value` argument parsing, mirroring the original scheme
//! rather than pulling in `clap` for a two-verb CLI (see migration notes).

use crate::error::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Backup,
    Restore,
}

#[derive(Debug, Clone)]
pub struct Args {
    pub action: Action,
    pub bucket: String,
    pub folder: Option<String>,
    pub object: Option<String>,
    pub config_path: Option<String>,
}

/// Parses `argv` (excluding the program name) into a flat `-key value` map,
/// then validates it into `Args`.
///
/// Requirements enforced here (from the requirements doc):
/// - any `-flag` with no following non-dash token is invalid
/// - `-action` is required and must be exactly `backup` or `restore`
/// - `backup` requires `-bucket` and `-folder`
/// - `restore` requires `-bucket`; `-object` is optional
pub fn parse(argv: &[String]) -> Result<Args, AppError> {
    let mut map: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    let mut i = 0;
    while i < argv.len() {
        let tok = &argv[i];
        if let Some(key) = tok.strip_prefix('-') {
            if key.is_empty() {
                return Err(AppError::Usage(format!("empty flag at position {i}")));
            }
            let next = argv.get(i + 1);
            match next {
                Some(v) if !v.starts_with('-') => {
                    map.insert(key.to_string(), v.clone());
                    i += 2;
                }
                _ => {
                    return Err(AppError::Usage(format!(
                        "flag -{key} requires a value"
                    )));
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
        other => {
            return Err(AppError::Usage(format!(
                "-action must be 'backup' or 'restore', got '{other}'"
            )))
        }
    };

    let bucket = map
        .remove("bucket")
        .ok_or_else(|| AppError::Usage("-bucket is required".into()))?;
    let folder = map.remove("folder");
    let object = map.remove("object");
    let config_path = map.remove("config");

    if action == Action::Backup && folder.is_none() {
        return Err(AppError::Usage("-folder is required for -action backup".into()));
    }

    Ok(Args {
        action,
        bucket,
        folder,
        object,
        config_path,
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
    fn backup_ok() {
        let a = parse(&v(&["-action", "backup", "-folder", "/tmp/x", "-bucket", "b"])).unwrap();
        assert_eq!(a.action, Action::Backup);
        assert_eq!(a.folder.as_deref(), Some("/tmp/x"));
        assert_eq!(a.bucket, "b");
    }

    #[test]
    fn restore_object_optional() {
        let a = parse(&v(&["-action", "restore", "-bucket", "b"])).unwrap();
        assert_eq!(a.action, Action::Restore);
        assert_eq!(a.object, None);
    }

    #[test]
    fn restore_with_object() {
        let a = parse(&v(&[
            "-action", "restore", "-bucket", "b", "-object", "host_user_path.tar.gz.enc",
        ]))
        .unwrap();
        assert_eq!(a.object.as_deref(), Some("host_user_path.tar.gz.enc"));
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
