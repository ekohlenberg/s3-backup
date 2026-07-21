//! s3b: archive, compress, client-side encrypt, and sync folders to an
//! S3-compatible bucket; restore uploaded objects back to a local temp
//! directory. Rust port of the original .NET `s3b` tool -- see
//! `docs/s3-backup-requirements.md` and `docs/Migration Changes.md` in the
//! project knowledge for the full requirements and rationale behind each
//! design change.

mod archive;
mod backup;
mod cli;
mod config;
mod crypto;
mod error;
mod hashing;
mod logging;
mod manifest;
mod naming;
mod restore;
mod s3;
mod time_util;

use error::AppError;

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();

    // Exceptions are logged, not rethrown to the shell (requirement 2): a
    // panic anywhere in the pipeline is caught here, logged, and turned into
    // a plain exit code 1 rather than an unfriendly Rust backtrace.
    let exit_code = match std::panic::catch_unwind(|| run(&argv)) {
        Ok(code) => code,
        Err(payload) => {
            logging::error(format!("unexpected internal error: {}", panic_message(&payload)));
            1
        }
    };

    std::process::exit(exit_code);
}

fn run(argv: &[String]) -> i32 {
    let args = match cli::parse(argv) {
        Ok(a) => a,
        Err(AppError::Usage(msg)) => {
            eprintln!("usage error: {msg}\n\n{}", error::USAGE);
            return 1;
        }
        Err(e) => {
            logging::error(format!("{e}"));
            return 1;
        }
    };

    // genkey needs neither a loaded Config (no AWS credentials, no public
    // key -- it's what *creates* the public key) nor a bucket, so it's
    // handled before Config::load rather than folded into the match below.
    if args.action == cli::Action::Genkey {
        // -out is optional; defaults to crypto::DEFAULT_KEY_PREFIX ("s3b"),
        // matching the ~/s3b.pub fallback backup uses when S3BPUBKEY is unset.
        let out = args.out.as_deref().unwrap_or(crypto::DEFAULT_KEY_PREFIX);
        return match crypto::genkey(out) {
            Ok(()) => 0,
            Err(e) => {
                logging::error(format!("{e}"));
                1
            }
        };
    }

    let cfg = match config::Config::load(args.config_path.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            logging::error(format!("{e}"));
            return 1;
        }
    };

    let result = match args.action {
        cli::Action::Backup => {
            // Validated by cli::parse: -folder and -bucket are required for backup.
            let folder = args.folder.as_deref().expect("cli::parse enforces -folder for backup");
            let bucket = args.bucket.as_deref().expect("cli::parse enforces -bucket for backup");
            backup::run(&cfg, folder, bucket, args.force)
        }
        cli::Action::Restore => {
            // Validated by cli::parse: -bucket and -key are required for restore.
            let bucket = args.bucket.as_deref().expect("cli::parse enforces -bucket for restore");
            let key_path = args.key.as_deref().expect("cli::parse enforces -key for restore");
            restore::run(&cfg, bucket, args.object.as_deref(), std::path::Path::new(key_path))
        }
        cli::Action::Genkey => unreachable!("genkey is handled above, before Config::load"),
    };

    match result {
        Ok(()) => 0,
        Err(e) => {
            logging::error(format!("{e}"));
            1
        }
    }
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}
