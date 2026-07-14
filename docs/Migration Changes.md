# Migration Changes — Porting s3b to Rust

## Background: simplifications identified in the current .NET implementation

**Collapse the pipeline into one step.** Right now archive → compress → encrypt are three separate external processes (tar, gzip, openssl), each writing an intermediate file to temp disk and needing its own cleanup and stage-tracking. Piping them together — or better, doing archive+compress+encrypt in-process with a streaming library — eliminates the intermediate `.tar` and `.tar.gz` files entirely and removes most of the `stage`/`status` state machine, since there's nothing left to resume mid-pipeline.

**Use an SDK/API instead of shelling out to the `aws` CLI.** The current design lists bucket contents and diffs by filename+size as a separate reconciliation pass. A direct S3 API call returns the uploaded object's ETag/checksum in the response, so success can be verified inline instead of re-listing the whole bucket and cross-referencing.

**Switch AES-256-CBC to AES-256-GCM.** GCM is authenticated encryption — it detects tampering/corruption on decrypt for free, removing the need for a separate size-comparison integrity check.

**Track changes by content hash, not folder-level mtime.** The current logic re-uploads an entire subfolder if any single file inside changed, using `LastWriteTime`, which is easy to spoof or lose on file copies. A hash of file metadata (or contents) is more reliable.

**Remove the local database entirely.** `local_folder` stage/status tracking exists to resume a multi-stage pipeline — unnecessary once upload is atomic. Reconciliation state can live as S3 object metadata (checksum, source path, hostname, backup time) instead of a local SQLite mirror that has to stay in sync with bucket reality. See the state-management section below for the concrete replacement.

---

## Goals for the Rust port

1. **Minimize dependencies** — prefer the standard library and a small number of well-scoped crates over heavyweight SDKs or frameworks; avoid pulling in an async runtime unless there's a real concurrency need.
2. **Bulletproof** — the tool should fail loudly and safely rather than silently continuing past errors, should never leave local or remote state half-written, and should be resistant to partial failures (crash mid-upload, network drop, disk full).
3. **No external process dependencies** — the .NET version shells out to `tar`, `gzip`, `openssl`, and `aws`, all of which must be separately installed and are a source of platform drift (the README's Cygwin note for Windows is a symptom of this). A Rust port should do archiving, compression, encryption, and S3 calls natively in-process.
4. **No local database** — see state management below.

## Proposed crate set (minimal)

| Concern | Crate | Why |
|---|---|---|
| CLI argument parsing | `pico-args` or hand-rolled | Avoids `clap`'s larger dependency tree for a two-verb CLI; hand-rolled parsing (mirroring the current `-key value` scheme) is also a reasonable zero-dependency option given how small the surface is. |
| Archiving | `tar` | Pure Rust, streams to/from any `Write`/`Read`, no system `tar` binary needed. |
| Compression | `flate2` (with the `miniz_oxide` pure-Rust backend, not the `zlib` C binding) | Avoids a system zlib dependency; keeps the whole toolchain pure Rust and easy to cross-compile. |
| Encryption | `aes-gcm` (RustCrypto) | Authenticated encryption, pure Rust, no OpenSSL linkage. |
| Key derivation | `argon2` | Derives an encryption key from the passphrase file contents + a stored salt, rather than using raw passphrase bytes directly as key material (see security notes below). |
| Hashing | `sha2` | For file/folder content hashes used in change detection and object metadata. |
| S3 access | Hand-rolled SigV4 signing over `ureq` (sync HTTP), or `rusty-s3` (a minimal request-signing crate that pairs with any HTTP client) | Avoids the official `aws-sdk-s3`, which pulls in an async runtime (`tokio`) and a large transitive dependency graph. A synchronous client is a better fit for a CLI batch tool with no need for concurrent I/O multiplexing. |
| Serialization | `serde` + `serde_json` | Needed for the manifest and S3-object-metadata JSON; this dependency pays for itself. |
| Error handling | `thiserror` for typed errors at module boundaries; `std::error::Error` elsewhere | Keeps error handling explicit and typed without a broad "catch-all" pattern. |
| Logging | `eprintln!`/`println!` with a small manual timestamp+level wrapper | Avoids pulling in `log` + `env_logger`/`tracing` for a CLI tool whose entire log output is "print what happened, plus a structured run summary." |

Explicitly **not** used: any ORM, SQLite, `tokio`/async, the official AWS SDK, or a general templating engine (the `$(key)` substitution system in the current code is replaced by typed config — see below).

## Configuration: typed, not templated

Replace the current stringly-typed `Config` dictionary + `$(key)` recursive template substitution with a plain `serde`-deserialized struct read from a small TOML or JSON file. The current design lets a config typo (a misspelled `$(key)`) fail silently or produce a malformed shell command at runtime; a typed struct makes that a compile-time or startup-time error instead. Since external commands are no longer being templated into shell argument strings (archiving/compression/encryption/upload all happen in-process), most of the templating engine's job disappears entirely — the only remaining variable content is things like bucket name and temp directory, which are just fields on the config struct.

## State management: no local database

State is derived from two places instead of a local SQLite file:

**S3 object metadata**, attached at upload time:

| Metadata key | Value | Purpose |
|---|---|---|
| `source-hash` | Hash of the folder's file manifest (`(relative_path, size, mtime)` tuples, or full content hash) | Detect real content changes without trusting mtime alone |
| `source-path` | Original absolute folder path | Enables restoring to the original location (a gap in the current tool) |
| `hostname` / `username` | Machine/user that produced the backup | Supports the existing multi-host naming convention explicitly rather than only via filename encoding |
| `backup-time` | ISO 8601 timestamp | Audit trail, replaces `backup_log.backup_time` |

A single list call against the bucket (returning ETags/sizes) plus reading this metadata on objects of interest answers "what's already backed up and is it current" with no local persisted table.

**A manifest object** in the bucket itself (e.g. `_s3b/manifest.json`), updated after each successful run, giving a fast human-readable summary without a full bucket scan:

```json
{
  "backup_sets": [
    {
      "root_folder_path": "/Users/eric/Documents",
      "hostname": "erics-mbp",
      "username": "eric",
      "last_backup_datetime": "2026-07-13T18:42:00Z"
    }
  ]
}
```

Storing this in the bucket rather than locally also fixes a real correctness gap in the current design: backups from multiple hosts into a shared bucket can't be consistently tracked by a per-machine local SQLite file, but a bucket-resident manifest is naturally shared.

If a lightweight per-file audit trail (replacing `backup_log`) is wanted, append one JSON line per file per run to a rolling log object in the bucket (`_s3b/audit/<date>.jsonl`) rather than reintroducing a database.

## Bulletproofing checklist

- **Atomic local writes.** Any local file the tool writes (temp encrypted archive before upload, cached manifest copy) should be written to a `.tmp` path and renamed into place — rename is atomic on POSIX filesystems, so a crash mid-write never leaves a corrupt file where a good one is expected.
- **Verify before deleting sources.** Never delete or consider a folder "backed up" until the upload response's ETag/checksum has been confirmed — mirrors the current reconciliation intent but makes it synchronous and inline rather than a separate best-effort pass that runs after the fact.
- **Explicit retry with backoff**, implemented as a small, testable state machine (not hidden inside a library's default retry policy) so retry behavior is visible in code and covered by unit tests.
- **Fail closed, not open.** The current `.NET` code frequently logs an error and continues (e.g. the `clean()` step logs "unable to clean" and moves on, `restore` continues to the next object after a sub-step failure). For a tool whose stated purpose is ransomware protection, a bulletproof version should default to stopping and surfacing a non-zero exit code on any step that didn't complete as expected, with continuation only where explicitly safe (e.g. skipping one already-verified-current folder is fine; skipping a failed upload is not).
- **Key derivation instead of raw passphrase bytes.** Derive the AES key from the passphrase file via Argon2id with a stored, versioned salt, rather than feeding the file's raw bytes directly to the cipher as the current OpenSSL invocation effectively does. This is more resistant to a weak or short passphrase file and allows future key rotation without re-deriving from scratch.
- **Round-trip self-check (optional but recommended).** After encrypting, immediately decrypt in memory and compare against a hash of the original plaintext before uploading, catching any local encryption bug before it corrupts a backup irrecoverably.
- **No silent success on partial restore.** Restore should track and report which objects failed to download/decrypt/expand and exit non-zero if any did, rather than only logging and moving to the next object as today.
- **Unit-testable core.** Because archiving/compression/encryption/hashing are all in-process (no shelling out), the core pipeline logic becomes pure functions over bytes/streams that can be unit tested without touching the filesystem or network — a meaningful reliability improvement over the current design, which has no automated tests and is difficult to test given its dependence on external processes and a live database.

## Net effect

No SQLite, no ORM, no shelled-out `tar`/`gzip`/`openssl`/`aws` processes, no template-substitution config engine, and no async runtime — replaced by a small set of pure-Rust crates, a typed config struct, and state that lives either in S3 object metadata or a small manifest object in the bucket. The result should have meaningfully fewer runtime dependencies than the .NET version (which requires four separate external binaries plus a .NET runtime plus SQLite) while being easier to test and harder to leave in a half-completed state after a failure.
