//! Bucket-resident manifest, replacing the local SQLite `backup_set` table.
//!
//! Stored as a single JSON object at `_s3b/manifest.json` in the target
//! bucket. Because it lives in the bucket rather than on any one machine, it
//! stays consistent across multiple hosts backing up into a shared bucket --
//! something a per-machine local SQLite file structurally cannot do (see
//! migration notes).

use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::s3::S3Client;

pub const MANIFEST_KEY: &str = "_s3b/manifest.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupSetEntry {
    pub root_folder_path: String,
    pub hostname: String,
    pub username: String,
    pub last_backup_datetime: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Manifest {
    pub backup_sets: Vec<BackupSetEntry>,
}

impl Manifest {
    /// Loads the manifest from the bucket, or returns an empty one if it
    /// doesn't exist yet (first-ever backup to this bucket).
    pub fn load(client: &S3Client) -> Result<Manifest, AppError> {
        match client.get_object(MANIFEST_KEY) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| AppError::Config(format!("corrupt bucket manifest: {e}"))),
            Err(AppError::S3NotFound) => Ok(Manifest::default()),
            Err(e) => Err(e),
        }
    }

    pub fn save(&self, client: &S3Client) -> Result<(), AppError> {
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| AppError::Config(format!("serializing manifest: {e}")))?;
        client.put_object(MANIFEST_KEY, &bytes, &[])?;
        Ok(())
    }

    /// Inserts or updates the entry keyed by `(root_folder_path, hostname,
    /// username)`, mirroring the original "keyed by root folder path --
    /// reused across runs, not duplicated" behavior of `backup_set`.
    pub fn upsert(
        &mut self,
        root_folder_path: &str,
        hostname: &str,
        username: &str,
        last_backup_datetime: &str,
    ) {
        if let Some(existing) = self.backup_sets.iter_mut().find(|e| {
            e.root_folder_path == root_folder_path && e.hostname == hostname && e.username == username
        }) {
            existing.last_backup_datetime = last_backup_datetime.to_string();
        } else {
            self.backup_sets.push(BackupSetEntry {
                root_folder_path: root_folder_path.to_string(),
                hostname: hostname.to_string(),
                username: username.to_string(),
                last_backup_datetime: last_backup_datetime.to_string(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_updates_existing_entry_in_place() {
        let mut m = Manifest::default();
        m.upsert("/data", "host1", "eric", "2026-07-01T00:00:00Z");
        m.upsert("/data", "host1", "eric", "2026-07-13T18:42:00Z");
        assert_eq!(m.backup_sets.len(), 1);
        assert_eq!(m.backup_sets[0].last_backup_datetime, "2026-07-13T18:42:00Z");
    }

    #[test]
    fn upsert_keeps_distinct_hosts_separate() {
        let mut m = Manifest::default();
        m.upsert("/data", "host1", "eric", "2026-07-13T18:42:00Z");
        m.upsert("/data", "host2", "eric", "2026-07-13T19:00:00Z");
        assert_eq!(m.backup_sets.len(), 2);
    }

    #[test]
    fn round_trips_through_json() {
        let mut m = Manifest::default();
        m.upsert("/Users/eric/Documents", "erics-mbp", "eric", "2026-07-13T18:42:00Z");
        let json = serde_json::to_string(&m).unwrap();
        let parsed: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.backup_sets[0].root_folder_path, "/Users/eric/Documents");
    }
}
