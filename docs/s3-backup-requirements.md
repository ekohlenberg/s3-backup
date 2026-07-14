
## 1. Purpose

s3b is a command-line tool that archives, compresses, and client-side encrypts the immediate subfolders of a local directory, uploads the results to an S3-compatible bucket, and can restore an uploaded object back to a local folder. It is positioned as ransomware protection: backups are encrypted before leaving the machine and verified against the bucket's actual contents after upload.

## 2. Command-Line Interface

Single entry point `s3b` with two supported actions, selected via `-action`:

```
s3b -action backup  -folder <backup_folder> -bucket <s3_bucket>
s3b -action restore -bucket <s3_bucket> [-object <object>]
```

Requirements:
- Arguments are parsed as `-key value` pairs; any `-flag` with no following non-dash token is invalid.
- `-action` is required and must be exactly `backup` or `restore`; any other value fails with a usage error.
- `backup` requires `-bucket` and `-folder`.
- `restore` requires `-bucket`; `-object` is optional — if omitted, restore processes every object in the bucket.
- Missing/invalid arguments raise a `UsageException` that prints the two-line usage string above and exits with code 1.
- Process exit code is 0 on success, 1 on any handled failure or unhandled exception (exceptions are logged, not rethrown to the shell).

## 3. Configuration

- Config is loaded once per run from `appsettings.json` (section `appsettings`), merged with CLI args into a single key/value store (`Config`, a `Dictionary<string,object>` subclass).
- All config string values support `$(key)` placeholder substitution, recursively resolved against the same config dictionary (`Template.eval`), so values can reference other values (e.g. `"$(temp)/$(archive.target)"`).
- A decryption/encryption passphrase file path is required for every run:
  - Read from environment variable `S3BPASSFILE`, falling back to `S3B-PASSFILE`.
  - In DEBUG builds only, falls back to `/Library/s3b/data/id_pass` if neither env var is set.
  - If no passfile path is resolved, or the file doesn't exist on disk, the run fails with an exception before any work starts.
- Two `appsettings*.json` variants exist (default and `appsettings.macos.json`), differing mainly in the DB/temp file paths (`/Library/s3b/...` vs `/tmp/s3b/...`) — implying deployment on macOS (and presumably Windows, since a Windows-specific `\\*` path convention appears in code, and the README notes Windows needs Cygwin/utilities for tar/gzip).
- Each pipeline step (archive, compress, encrypt, upload, recon, download, decrypt, decompress, expand, listobj) has its own `<step>.enabled` flag, `<step>.command`, and `<step>.args` template — steps are external OS processes, not built-in logic, invoked via `ProcExec` with output captured line-by-line and tokenized on whitespace.

## 4. Backup Requirements

Triggered by `-action backup`. Behavior (`Backup.run`):

1. Clear the `message_log` table at the start of every backup run (log history is not retained across runs).
2. Resolve the given `-folder` to an absolute path and register/update it as a `backup_set` (keyed by root folder path — reused across runs, not duplicated).
3. Enumerate immediate child directories of the backup folder; register each as a `local_folder` (recursive scan enabled) plus the root folder itself as one more `local_folder` (non-recursive, i.e. only files directly in the root, not its subfolders' contents twice).
4. For every registered folder, scan its files (recursively for child folders, non-recursively for the root) and compare each file's last-write time against the folder's last successful `upload_datetime`. If any file is newer, the folder is flagged as "changed" and added to the working set for this run; a `backup_log` row is recorded per changed file capturing hostname, username, target bucket, parent folder, file path, backup time, file's last-write time, and the folder's prior upload time.
5. For each folder in the working set, run a five-stage pipeline, each stage independently toggle-able via config and skipped if the folder's current `stage` indicates it's already past that point:
   - **Archive**: tar the folder (recursive) or each individual file (non-recursive/root case) into a per-machine/per-user/per-path-named archive (`getArchiveName()`: `<hostname>_<username>_<folder_path>` with path separators/colons/spaces sanitized to underscores).
   - **Compress**: gzip the archive.
   - **Encrypt**: AES-256-CBC encrypt (openssl) using the passphrase file; records the resulting encrypted file's name and byte size against the folder record.
   - **Upload**: push the encrypted file to the target bucket (aws CLI `s3 cp`); stamps the folder's `upload_datetime` on completion (success or failure — this is recorded regardless of the exec return code).
   - **Clean**: delete the intermediate compressed and encrypted temp files (archive/tar file cleanup is intentionally skipped — comment notes gzip already removes its source).
   - Each stage's outcome (`in_progress`/`complete`/`error`) and current stage name are persisted to the folder record after every stage.
   - After all stages, the file-level `backup_log` rows collected for that folder are written, with `last_upload_time` updated to the folder's new upload timestamp.
6. **Reconciliation**: after processing, list actual bucket objects (`aws s3 ls`) and compare each uploaded folder's recorded encrypted file size against the live object's size. Any mismatch (or object simply not present) is logged as an error and the affected folder is re-added to the working set. If no local match exists for a listed remote object, it's logged and skipped (informational, not an error).
7. The whole build/process/reconcile cycle repeats — re-running the pipeline for any folders that failed reconciliation — up to 3 retries (4 total attempts) or until reconciliation passes clean.
8. On completion, the backup set's `last_backup_datetime` is stamped, regardless of whether any folders remained in error.

Implied requirements not fully realized in code:
- A `local_file` / file-level change-tracking table and a "newer files" SQL query (`s3bSqliteTemplate`) exist in the schema/template layer but the file-change detection actually implemented in `Backup.cs` (via `FileInfo.LastWriteTime` vs. folder `upload_datetime`) does not use them — this looks like a partially-migrated or abandoned finer-grained (per-file, not per-folder) change-detection design.
- `exclude` column on `local_file` suggests an intended (but unimplemented) file-exclusion feature.

## 5. Restore Requirements

Triggered by `-action restore`. Behavior (`Restore.run`):

1. List all objects in the target bucket (`aws s3 ls`, via `ListObj`).
2. For each listed object, if `-object` was supplied, only process the matching object name; if omitted, process every object.
3. For each matching object, derive its "base" name (encrypted filename with `.enc`, `.gz`, `.tar` suffixes stripped) and run, in order: Download → Decrypt → Decompress → Expand.
4. Each of these four sub-steps is itself gated by its own `<step>.enabled` config flag and is otherwise a thin wrapper that shells out per its configured command/args template (download via `aws s3 cp`, decrypt via `openssl enc -d`, decompress via `gzip -d`, expand via `tar xvf` into the temp directory) — the same generic `Job.exec` external-process mechanism used by backup.
5. Restore of one object is aborted (remaining sub-steps skipped) as soon as one sub-step fails, but processing continues to the next matching object in the listing.
6. Overall restore success/failure reflects both the initial listing call and every sub-step run.

Note: extracted files land in the shared temp directory (`s3b.temp`), not back into the original source folder location — there is no code that relocates/restores files to their original path; the user is expected to move them manually after expansion.

## 6. Data / Persistence Requirements

- SQLite is the default/implemented persistence backend (`SqlitePersist`); a parallel `SqlPersist` (SQL Server) implementation exists with matching SQL-generation logic but no working template dictionary is wired up in `Program.cs` (only `SqlitePersist` is instantiated) — SQL Server support appears scaffolded but not completed/selectable at runtime.
- Schema (SQLite): `backup_set` (one row per unique root folder), `local_folder` (one row per top-level folder + root, tracking pipeline stage/status/last_error/encrypted file name & size/upload timestamp), `backup_log` (one row per file included in an upload, historical audit trail — not cleared between runs, unlike `message_log`), `message_log` (run-scoped operational log), and a superseded `local_file` table (dropped/recreated across migration scripts, seemingly retired in favor of the coarser folder-level tracking) plus an `app_version` table for schema versioning.
- All model objects (`BackupSet`, `LocalFolder`, `BackupLog`, `MessageLog`, `ObjectInfo`) are simple property-bag `Dictionary`-backed records with dirty-tracking so that SQL `UPDATE` statements only touch changed columns.
- Generic persistence layer supports get-by-id, "upsert by unique column" (`put`), insert, update, and templated/filtered select, with SQL built by naive string concatenation and quoting (no parameterized queries) — a security/robustness gap for any config or data containing unescaped quotes beyond the basic `'` doubling handled in `toSql`.

## 7. Logging Requirements

- All log lines (`info`, `warn`, `error`, plus multi-line/exception overloads) are written to the `message_log` table and echoed to stdout via `Console.WriteLine`.
- `message_log` is truncated at the start of every backup run, so historical run logs are not retained in the DB (only whatever's captured in redirected stdout/console, if any, persists longer-term).
- `debug` logging exists as a no-op stub (implemented but commented out) — hook is present for future use.

## 8. External Dependencies

The tool is a thin orchestrator around external OS binaries, all invoked by shelling out (`System.Diagnostics.Process`) with no built-in fallback if a tool is missing:
- `tar` — archiving (and un-archiving on restore).
- `gzip` — compression/decompression.
- `openssl` — AES-256-CBC symmetric encryption/decryption, keyed by a local passphrase file (`enc -pass file:<passfile>`).
- `aws` CLI — S3 upload, listing, and download; README notes it's only been tested against AWS S3 though other providers' CLIs could theoretically be substituted via config.
- SQLite (via `Microsoft.Data.Sqlite`) for the database, with an optional external DB browser for inspection.
- On Windows, `tar`/`gzip` require Cygwin or equivalent Unix-tool ports (per README) since they aren't native.

## 9. Non-Functional / Implicit Requirements

- **Idempotency / resumability**: folder-level `stage`/`status` tracking lets a rerun skip already-completed pipeline stages for a folder that failed partway through (`getStageCode()` maps `stage` name to the bitmask of remaining stages).
- **Verification**: uploads aren't trusted as successful just because the CLI call returned 0 — actual bucket state is listed and compared by file size as a integrity check, with automatic retry (up to 3x) of any folder that fails verification.
- **Multi-host awareness**: hostname and username are embedded in archive/object names and logged per file, implying the tool is expected to run from multiple machines/users against potentially shared buckets without name collisions.
- **Granularity**: backup and change-detection operate at the top-level-subfolder granularity, not per-file — an entire subfolder is re-archived/re-uploaded if any single file inside it changed. This is a scalability/cost consideration for large or deep folder trees.
- **No encryption-key management beyond a flat passphrase file**: the tool assumes the passphrase file already exists and is provisioned out-of-band; there's no key generation, rotation, or multi-recipient support.
- **Restore is destination-agnostic**: restore only stages decrypted/expanded files in the temp directory; there's no requirement implemented for restoring to the original path or a user-specified destination.

## 10. Notable Gaps / Inconsistencies Found in Code

- `s3bMSSqlTemplate` and `SqlPersist` exist but are dead code paths — `Program.cs` hardcodes `SqlitePersist`; SQL Server is not currently a selectable backend despite being implemented.
- `local_file` table and the "newer" SQL query in `s3bSqliteTemplate` suggest an original design for per-file change tracking that was replaced by the simpler per-folder `LastWriteTime`-vs-`upload_datetime` comparison actually used in `Backup.cs`; the old table/query are now unused/orphaned.
- Windows path literal (`fldr.folder_path + "\\*"`) is hardcoded in `Backup.setParameters`, unused elsewhere — remnant of a Windows-first design not fully reconciled with the macOS-only `appsettings.macos.json` cleanup.
- `appsettings.macos.json` is out of sync with the default `appsettings.json` (missing `recon.target`/`recon.output`, uses different/older restore template keys like `$(object)` instead of `$(encrypted_base_file_name)`, and has a `decompress.args` referencing an undefined `$(decrypt.archive)`) — restore would likely fail against this config as written.
- `Config.setValue("temp", ...)` is set once from `s3b.temp` during arg parsing but nothing prevents it from being stale if `s3b.temp` were reassigned later.

## 11. Summary of Configurable Behavior

| Config key prefix | Controls |
|---|---|
| `archive.*` | tar command/args/target/cleanup |
| `compress.*` | gzip command/args/target/cleanup |
| `encrypt.*` | openssl encryption command/args/target/cleanup |
| `upload.*` | aws s3 cp upload command/args |
| `recon.*` | aws s3 ls reconciliation command/args |
| `download.*` | aws s3 cp download (restore) command/args |
| `decrypt.*` | openssl decryption (restore) command/args |
| `decompress.*` | gzip -d (restore) command/args |
| `expand.*` | tar x (restore) command/args |
| `listobj.*` | aws s3 ls (restore listing) command/args |
| `db.connection` | SQLite connection string |
| `s3b.temp` | working/scratch directory for all intermediate files |
