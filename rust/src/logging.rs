//! Deliberately tiny logger: local-time timestamp + level + message to
//! stdout (stderr for ERROR), mirrored to a per-session log file.
//!
//! The .NET version wrote every log line to a `message_log` SQLite table,
//! truncated at the start of each run, as well as to stdout. This port has
//! no local database (see the migration notes), so a truncated-per-session
//! log file (`~/.s3b/s3b.log`, opened once by `init()`) plays that role
//! instead -- only the latest session's log is kept, there's no rotation --
//! and a run summary is still printed at the end as a single-line "what
//! happened" record.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::crypto::DEFAULT_KEY_DIR;
use crate::time_util::local_iso8601_now;

const LOG_FILENAME: &str = "s3b.log";

/// `None` until `init()` runs (or if it failed to open the file); logging
/// then falls back to console-only silently for every subsequent call.
static LOG_FILE: Mutex<Option<File>> = Mutex::new(None);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

/// Opens `~/.s3b/s3b.log` for this session, truncating whatever the
/// previous session left behind (creating `~/.s3b` first if it doesn't
/// exist yet). Call once, as early as possible in `main.rs`, before any
/// other logging happens -- log calls before `init()` runs, or if it
/// fails, still print to the console, they just aren't mirrored to the
/// file.
///
/// Failing to open the log file (no home directory resolvable, permission
/// error, disk full, etc.) is not fatal: logging is a diagnostic
/// convenience, not a correctness requirement, so this prints one console
/// warning explaining why and continues with console-only logging rather
/// than aborting the run.
pub fn init() {
    match log_file_path() {
        Some(path) => match open_log_file(&path) {
            Ok(file) => {
                *LOG_FILE.lock().expect("log file mutex poisoned") = Some(file);
            }
            Err(e) => {
                eprintln!(
                    "warning: could not open log file {}: {e} (continuing with console-only logging)",
                    path.display()
                );
            }
        },
        None => {
            eprintln!(
                "warning: no home directory could be resolved to determine the log file \
                 location (set HOME or USERPROFILE); continuing with console-only logging"
            );
        }
    }
}

fn open_log_file(path: &Path) -> std::io::Result<File> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
}

/// `~/.s3b/s3b.log` under the given home directory.
fn log_file_path_from_home(home: &Path) -> PathBuf {
    home.join(DEFAULT_KEY_DIR).join(LOG_FILENAME)
}

/// `~/.s3b/s3b.log` (`%USERPROFILE%\.s3b\s3b.log` on Windows), or `None` if
/// neither `HOME` nor `USERPROFILE` is set. Mirrors `crypto::default_key_dir`
/// / `config`'s identical env-var fallback order.
fn log_file_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| log_file_path_from_home(Path::new(&home)))
}

pub fn log(level: Level, msg: impl AsRef<str>) {
    // Local time (not UTC) for the human reading this line -- see
    // time_util's module doc for why object metadata/the manifest stay UTC.
    let line = format!("{} [{}] {}", local_iso8601_now(), level.label(), msg.as_ref());
    match level {
        Level::Error => eprintln!("{line}"),
        _ => println!("{line}"),
    }

    // Best-effort mirror to the session log file: a poisoned mutex or a
    // failed write shouldn't take down the run, so both are silently
    // swallowed here rather than propagated.
    if let Ok(mut guard) = LOG_FILE.lock() {
        if let Some(file) = guard.as_mut() {
            let _ = writeln!(file, "{line}");
        }
    }
}

pub fn info(msg: impl AsRef<str>) {
    log(Level::Info, msg);
}

pub fn warn(msg: impl AsRef<str>) {
    log(Level::Warn, msg);
}

pub fn error(msg: impl AsRef<str>) {
    log(Level::Error, msg);
}

/// A tally of what happened this run, printed as the closing summary line so
/// there's always a single-line answer to "did it work" without a DB to
/// query afterwards.
#[derive(Debug, Default)]
pub struct RunSummary {
    pub succeeded: usize,
    pub failed: usize,
    pub skipped_unchanged: usize,
}

impl RunSummary {
    pub fn print(&self, action: &str) {
        info(format!(
            "{action} summary: {} succeeded, {} failed, {} skipped (unchanged)",
            self.succeeded, self.failed, self.skipped_unchanged
        ));
    }

    pub fn is_clean(&self) -> bool {
        self.failed == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_file_path_from_home_is_dot_s3b_s3b_log() {
        let p = log_file_path_from_home(Path::new("/home/eric"));
        assert_eq!(p, PathBuf::from("/home/eric/.s3b/s3b.log"));
    }

    #[test]
    fn open_log_file_creates_parent_dir_and_truncates_on_reopen() {
        let dir = tempfile::tempdir().unwrap();
        // Nested to also exercise the create_dir_all call.
        let path = dir.path().join("nested").join("s3b.log");

        {
            let mut f = open_log_file(&path).unwrap();
            writeln!(f, "first session line").unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "first session line\n"
        );

        // Re-opening, as init() does at the start of a new session, must
        // truncate the previous session's content rather than appending --
        // only the latest session's log is meant to be kept.
        {
            let mut f = open_log_file(&path).unwrap();
            writeln!(f, "second session line").unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "second session line\n"
        );
    }
}
