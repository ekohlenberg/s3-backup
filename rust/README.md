# s3b (Rust port)

Rust port of the .NET `s3b` command-line tool: archives, compresses, and
client-side encrypts the immediate subfolders of a local directory, uploads
the results to an S3-compatible bucket, and can restore an uploaded object
back to a local temp directory.

This is a from-scratch reimplementation, not a line-by-line translation --
see "What changed from the .NET version" below for the rationale, taken from
the project's migration notes.

## Build

```sh
cargo build --release
```

The binary is `target/release/s3b` (`s3b.exe` on Windows).

Builds and runs on macOS, Linux, and Windows -- every dependency is pure
Rust (no OpenSSL, no system zlib, no `tar`/`gzip`/`aws` CLI), so there's
nothing platform-specific to install first.

## Usage

```
s3b -action backup  -folder <backup_folder> [-bucket <s3_bucket>] [-force]
s3b -action restore [-bucket <s3_bucket>] [-key <private_key_file>] [-object <object>]
s3b -action genkey  [-out <key_prefix>]
```

`-bucket` and `-key` are optional -- see the fallback files below.

Required, from the environment or `~/.s3b/s3b.aws`:

- `AWS_ACCESS_KEY_ID` (or the shorter `AWS_ACCESS_KEY`), `AWS_SECRET_ACCESS_KEY`
  -- S3 credentials. `AWS_SESSION_TOKEN` is read from the environment only.

Optional, from the environment or `~/.s3b/s3b.aws`:

- `AWS_REGION` / `AWS_DEFAULT_REGION` -- defaults to `us-east-1`.

`~/.s3b/s3b.aws` (`%USERPROFILE%\.s3b\s3b.aws` on Windows) is a fallback file
for anything not set in the environment or on the command line, `key=value`
per line:

```
AWS_ACCESS_KEY_ID=AKIA...
AWS_SECRET_ACCESS_KEY=...
AWS_REGION=us-east-1
BUCKET=my-bucket
```

`BUCKET` there is used whenever `-bucket` is omitted on the command line.

Optional config file (TOML), path via `-config <path>`, or `./s3b.toml` if
present:

```toml
temp_dir = "/tmp/s3b"     # defaults to the OS temp dir + "s3b" if omitted
region = "us-east-1"
s3_endpoint = "https://s3.us-west-000.backblazeb2.com"  # for non-AWS S3-compatible providers
retry_attempts = 3
hostname = "erics-mbp"   # override; defaults to the OS hostname
username = "eric"        # override; defaults to $USER
```

`hostname` is resolved from (in order): this config field, `$HOSTNAME` /
`%COMPUTERNAME%`, `/etc/hostname` (Linux), then the `hostname` command
(covers macOS, which doesn't populate `/etc/hostname`). `username` comes
from this config field, then `$USER` / `%USERNAME%`.

### Platform notes

- `temp_dir` defaults to the OS temp directory (`$TMPDIR`/`/tmp` on
  Unix, `%TEMP%` on Windows) with an `s3b` subfolder -- override it in
  `s3b.toml` if you want it somewhere specific.
- Object names (built from hostname/username/folder path) are sanitized to
  be valid as both S3 keys and local filenames, since they're also used as
  temp file names during backup/restore -- so they're safe under Windows'
  stricter filename rules (`< > : " / \ | ? *` and control characters are
  all replaced with `_`), not just S3's more permissive ones.

## What changed from the .NET version

This follows the migration plan agreed for this port:

- **No external process dependencies.** The .NET version shells out to
  `tar`, `gzip`, `openssl`, and the `aws` CLI. This port does archiving
  (`tar` crate), compression (`flate2`, pure-Rust backend), encryption
  (`aes-gcm`), and S3 calls (hand-rolled SigV4 signing over `ureq`) natively,
  with no system binaries required and no async runtime.
- **AES-256-GCM instead of AES-256-CBC.** Authenticated encryption detects
  tampering/corruption on decrypt for free.
- **Argon2id key derivation** from the passphrase file's contents, instead of
  feeding raw passphrase bytes straight into the cipher.
- **Content-hash change detection** (a hash of each folder's
  `(relative_path, size, mtime)` manifest) instead of comparing
  `LastWriteTime` against a stored `upload_datetime`.
- **No local database.** State that used to live in SQLite now lives either
  as S3 object metadata (`source-hash`, `source-path`, `hostname`,
  `username`, `backup-time`, stamped on each uploaded object) or in a small
  bucket-resident manifest at `_s3b/manifest.json` -- which also fixes a real
  gap in the old design, since a per-machine SQLite file can't consistently
  track backups from multiple hosts into a shared bucket.
- **Inline upload verification** via the PUT response's ETag, instead of a
  separate `aws s3 ls` reconciliation pass run after the fact.
- **Fail closed.** A folder is never considered backed up until upload
  verification succeeds; restore tracks and reports every object that failed
  instead of only logging and moving on.

## Layout

| File | Purpose |
|---|---|
| `src/main.rs` | Entry point, panic handling, exit codes |
| `src/cli.rs` | `-key value` argument parsing |
| `src/config.rs` | Typed config (TOML) + environment resolution |
| `src/backup.rs` | Backup orchestration (enumerate, hash, pipeline, retry) |
| `src/restore.rs` | Restore orchestration (list, download, decrypt, expand) |
| `src/archive.rs` | Streaming tar+gzip archive / expand |
| `src/crypto.rs` | Argon2id key derivation + AES-256-GCM encrypt/decrypt |
| `src/hashing.rs` | Folder content-hash + SHA-256 (upload checksum verification) |
| `src/naming.rs` | Object key naming convention |
| `src/manifest.rs` | Bucket-resident `_s3b/manifest.json` |
| `src/s3/` | Hand-rolled SigV4-signed S3 client (PUT/GET/HEAD/List) |
| `src/logging.rs` | Minimal timestamped logger + run summary |
| `src/time_util.rs` | Dependency-free UTC date/time formatting |
