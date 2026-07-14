//! Deliberately tiny logger: timestamp + level + message to stdout.
//!
//! The .NET version wrote every log line to a `message_log` SQLite table
//! (truncated at the start of each run) as well as to stdout. This port has
//! no local database (see the migration notes), so stdout is the log; a run
//! summary is printed at the end as the durable "what happened" record
//! instead of a DB table.

use crate::time_util::iso8601_now;

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

pub fn log(level: Level, msg: impl AsRef<str>) {
    let line = format!("{} [{}] {}", iso8601_now(), level.label(), msg.as_ref());
    match level {
        Level::Error => eprintln!("{line}"),
        _ => println!("{line}"),
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
