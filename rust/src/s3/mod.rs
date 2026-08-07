//! Hand-rolled, synchronous S3 client (SigV4 signing over `ureq`), replacing
//! the `aws` CLI shell-out entirely. Deliberately implements only the four
//! operations this tool needs: PUT (upload), GET (download), HEAD (read
//! metadata without downloading the body), and ListObjectsV2 (paginated).
//!
//! No async runtime: this is a synchronous, one-request-at-a-time client, a
//! deliberate fit for a CLI batch tool per the migration notes.

mod sigv4;
mod xml;

pub use xml::ObjectSummary;

use std::collections::BTreeMap;
use std::io::Read;
use std::time::Duration;

use crate::config::Config;
use crate::error::AppError;
use crate::hashing;
use crate::logging::{info, warn};
use crate::time_util::amz_date_now;

pub struct PutResult {
    pub etag: String,
    /// The `x-amz-checksum-sha256` value S3 echoes back in the PUT response,
    /// confirming the digest it validated the uploaded bytes against. For a
    /// multipart upload this is the *composite* checksum (SHA-256 of the
    /// concatenated per-part digests, suffixed `-<part_count>`), not a plain
    /// whole-body SHA-256 -- see `verified`.
    pub checksum_sha256: Option<String>,
    /// True when this client already validated the upload's integrity
    /// itself before returning (currently: the multipart path, which checks
    /// the composite checksum against its own per-part digests before
    /// `upload_object` returns). Callers that otherwise compare
    /// `checksum_sha256` against a plain whole-body SHA-256 (as `backup.rs`
    /// does) must skip that comparison when this is true, since the
    /// composite format isn't a whole-body digest.
    pub verified: bool,
}

/// Mirrors the full set of metadata keys `backup.rs` writes on upload (see
/// the migration notes' "S3 object metadata" table). Only `source_hash` is
/// currently consulted (for change detection), but the rest is parsed here
/// too so it's available to any future caller -- e.g. a `-action info`
/// command to show who backed up a folder and when -- without changing the
/// HEAD-parsing code again.
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct ObjectMetadata {
    pub source_hash: Option<String>,
    pub source_path: Option<String>,
    pub hostname: Option<String>,
    pub username: Option<String>,
    pub backup_time: Option<String>,
    pub size: u64,
}

/// S3's own minimum: every part but the last in a multipart upload must be
/// at least this large, or `CompleteMultipartUpload` rejects the request.
const MIN_MULTIPART_PART_SIZE: usize = 5 * 1024 * 1024;

pub struct S3Client {
    bucket: String,
    region: String,
    host: String,
    base_url: String,
    path_style: bool,
    access_key: String,
    secret_key: String,
    session_token: Option<String>,
    agent: ureq::Agent,
}

impl S3Client {
    pub fn new(cfg: &Config, bucket: &str) -> S3Client {
        let (host, base_url, path_style) = match &cfg.s3_endpoint {
            Some(endpoint) => {
                let trimmed = endpoint.trim_end_matches('/').to_string();
                let host = trimmed
                    .splitn(2, "://")
                    .nth(1)
                    .unwrap_or(&trimmed)
                    .to_string();
                (host, trimmed, true)
            }
            None => {
                let host = format!("{bucket}.s3.{}.amazonaws.com", cfg.region);
                let base = format!("https://{host}");
                (host, base, false)
            }
        };

        // `max_idle_connections_per_host(0)` disables ureq's connection
        // pooling/reuse for this agent (default is 1 idle connection kept
        // per host). A large multipart upload makes hundreds to thousands
        // of sequential requests to the same host; some Windows AV/firewall
        // "network protection" modules single out long-lived reused
        // connections and kill them after enough traffic passes through --
        // exactly the `os error 10053` transport errors seen testing large
        // uploads. Opening a fresh TCP+TLS connection per request costs a
        // bit of latency but sidesteps that heuristic entirely; combined
        // with the retry logic in `upload_object`, a single connection kill
        // now costs one retried request instead of the whole upload.
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout(Duration::from_secs(300))
            .max_idle_connections_per_host(0)
            .build();

        S3Client {
            bucket: bucket.to_string(),
            region: cfg.region.clone(),
            host,
            base_url,
            path_style,
            access_key: cfg.aws_access_key_id.clone(),
            secret_key: cfg.aws_secret_access_key.clone(),
            session_token: cfg.aws_session_token.clone(),
            agent,
        }
    }

    /// Returns `(full_url, canonical_uri)` for `key`. `canonical_uri` is
    /// already percent-encoded and is used both to build the actual request
    /// URL and, verbatim, in the SigV4 canonical request (S3 does not get
    /// double-encoded).
    fn object_uri(&self, key: &str) -> String {
        let encoded_key = sigv4::uri_encode_path(key);
        if self.path_style {
            format!("/{}/{}", sigv4::uri_encode_path(&self.bucket), encoded_key)
        } else {
            format!("/{encoded_key}")
        }
    }

    fn full_url(&self, canonical_uri: &str, query: &str) -> String {
        if query.is_empty() {
            format!("{}{}", self.base_url, canonical_uri)
        } else {
            format!("{}{}?{}", self.base_url, canonical_uri, query)
        }
    }

    fn base_headers(&self, payload_hash: &str, amz_date: &str) -> BTreeMap<String, String> {
        let mut h = BTreeMap::new();
        h.insert("host".to_string(), self.host.clone());
        h.insert("x-amz-content-sha256".to_string(), payload_hash.to_string());
        h.insert("x-amz-date".to_string(), amz_date.to_string());
        if let Some(tok) = &self.session_token {
            h.insert("x-amz-security-token".to_string(), tok.clone());
        }
        h
    }

    /// Executes one signed HTTP call. `extra_headers` are added to both the
    /// signature and the outgoing request (already lowercase names).
    fn execute(
        &self,
        method: &str,
        key_or_prefix_uri: &str,
        query: &BTreeMap<String, String>,
        mut extra_headers: BTreeMap<String, String>,
        body: &[u8],
    ) -> Result<ureq::Response, AppError> {
        let (amz_date, date_stamp) = amz_date_now();
        let payload_hash = if body.is_empty() {
            sigv4::EMPTY_PAYLOAD_SHA256.to_string()
        } else {
            sigv4::sha256_hex(body)
        };

        let mut headers = self.base_headers(&payload_hash, &amz_date);
        headers.append(&mut extra_headers);

        let canonical_query = sigv4::canonical_query_string(query);

        let signed = sigv4::sign(&sigv4::SigningInput {
            method,
            canonical_uri: key_or_prefix_uri,
            canonical_query_string: &canonical_query,
            headers: &headers,
            payload_sha256_hex: &payload_hash,
            region: &self.region,
            access_key: &self.access_key,
            secret_key: &self.secret_key,
            amz_date: &amz_date,
            date_stamp: &date_stamp,
        });

        let url = self.full_url(key_or_prefix_uri, &canonical_query);
        let mut req = self.agent.request(method, &url);
        for (k, v) in headers.iter() {
            if k == "host" {
                continue; // ureq sets Host itself from the URL
            }
            req = req.set(k, v);
        }
        req = req.set("Authorization", &signed.authorization_header);

        let result = if body.is_empty() {
            req.call()
        } else {
            req.send_bytes(body)
        };

        result.map_err(map_ureq_error)
    }

    pub fn put_object(
        &self,
        key: &str,
        body: &[u8],
        metadata: &[(&str, &str)],
    ) -> Result<PutResult, AppError> {
        let uri = self.object_uri(key);
        let mut extra = BTreeMap::new();
        for (k, v) in metadata {
            extra.insert(format!("x-amz-meta-{}", k.to_lowercase()), v.to_string());
        }
        // Ask S3 to validate the upload against a client-computed SHA-256
        // digest -- S3 rejects the PUT outright if the bytes it received
        // don't match, so this catches in-flight corruption before the
        // object is even considered written, not just after the fact.
        extra.insert(
            "x-amz-checksum-sha256".to_string(),
            hashing::sha256_base64(body),
        );
        let resp = self.execute("PUT", &uri, &BTreeMap::new(), extra, body)?;
        let etag = resp
            .header("ETag")
            .unwrap_or_default()
            .trim_matches('"')
            .to_string();
        let checksum_sha256 = resp.header("x-amz-checksum-sha256").map(|s| s.to_string());
        Ok(PutResult {
            etag,
            checksum_sha256,
            verified: false,
        })
    }

    /// Uploads `body` to `key`, transparently switching from a single PUT to
    /// S3 multipart upload once `body` exceeds `threshold` bytes (split into
    /// `part_size`-byte parts, `MIN_MULTIPART_PART_SIZE` at minimum -- S3
    /// rejects any non-final part smaller than 5 MiB).
    ///
    /// Splitting large uploads into independently-retriable parts is a
    /// direct fix for connection resets seen uploading large backups over
    /// flaky network paths (e.g. antivirus/firewall software killing
    /// long-lived HTTPS connections on Windows): each part is its own
    /// request, so a single aborted part costs a retry of that part, not
    /// the whole file, and each request is small enough to usually finish
    /// before whatever is killing long-lived connections gets the chance.
    ///
    /// `part_retry_attempts` is how many *additional* attempts each
    /// individual network call in this path gets beyond the first (applied
    /// to every part upload and to the create/complete calls that bookend
    /// them) -- with a large file split into hundreds or thousands of
    /// parts, even a low per-request failure rate becomes likely to hit
    /// *something* over the whole upload if no single request gets a second
    /// try, so retrying at the part level (cheap: one part, not the whole
    /// file) rather than only at the whole-folder level (expensive: a full
    /// re-archive/re-encrypt/re-upload from byte zero) is what actually
    /// makes very large uploads reliable.
    pub fn upload_object(
        &self,
        key: &str,
        body: &[u8],
        metadata: &[(&str, &str)],
        threshold: usize,
        part_size: usize,
        part_retry_attempts: u32,
    ) -> Result<PutResult, AppError> {
        if body.len() <= threshold {
            return self.put_object(key, body, metadata);
        }

        let part_size = part_size.max(MIN_MULTIPART_PART_SIZE);
        let max_attempts = part_retry_attempts.saturating_add(1);

        let upload_id = with_retry(max_attempts, &format!("create-multipart-upload for {key}"), || {
            self.create_multipart_upload(key, metadata)
        })?;

        match self.upload_parts_and_complete(key, &upload_id, body, part_size, max_attempts) {
            Ok(result) => Ok(result),
            Err(e) => {
                // Best-effort cleanup so a failed upload doesn't leave
                // orphaned parts billing for storage indefinitely. The
                // abort's own outcome is deliberately not surfaced here --
                // the caller needs to see and act on the original failure,
                // not a secondary one from cleanup.
                let _ = self.abort_multipart_upload(key, &upload_id);
                Err(e)
            }
        }
    }

    fn upload_parts_and_complete(
        &self,
        key: &str,
        upload_id: &str,
        body: &[u8],
        part_size: usize,
        max_attempts: u32,
    ) -> Result<PutResult, AppError> {
        let mut parts = Vec::new();
        for (i, chunk) in body.chunks(part_size).enumerate() {
            let part_number = (i + 1) as u32;
            let part = with_retry(
                max_attempts,
                &format!("upload of part {part_number} of {key}"),
                || self.upload_part(key, upload_id, part_number, chunk),
            )?;
            parts.push(part);
        }

        with_retry(
            max_attempts,
            &format!("complete-multipart-upload for {key}"),
            || self.complete_multipart_upload(key, upload_id, &parts),
        )
    }

    /// Starts a multipart upload, returning the upload ID S3 assigns. Object
    /// metadata (`x-amz-meta-*`) is fixed at creation for a multipart
    /// upload -- there's no later step where it could be attached instead --
    /// so it's passed here rather than at `complete_multipart_upload`.
    fn create_multipart_upload(
        &self,
        key: &str,
        metadata: &[(&str, &str)],
    ) -> Result<String, AppError> {
        let uri = self.object_uri(key);
        let mut extra = BTreeMap::new();
        for (k, v) in metadata {
            extra.insert(format!("x-amz-meta-{}", k.to_lowercase()), v.to_string());
        }
        extra.insert("x-amz-checksum-algorithm".to_string(), "SHA256".to_string());

        let mut query = BTreeMap::new();
        query.insert("uploads".to_string(), String::new());

        let resp = self.execute("POST", &uri, &query, extra, &[])?;
        let body = resp
            .into_string()
            .map_err(|e| AppError::S3(format!("reading create-multipart-upload response: {e}")))?;
        xml::parse_upload_id(&body)
    }

    /// Uploads one part, verifying inline that the `x-amz-checksum-sha256`
    /// S3 echoes back for this part matches what was sent -- the same
    /// "reject on mismatch" philosophy `put_object` uses for a whole-body
    /// upload, applied per part.
    fn upload_part(
        &self,
        key: &str,
        upload_id: &str,
        part_number: u32,
        part: &[u8],
    ) -> Result<xml::CompletedPart, AppError> {
        let uri = self.object_uri(key);
        let mut query = BTreeMap::new();
        query.insert("partNumber".to_string(), part_number.to_string());
        query.insert("uploadId".to_string(), upload_id.to_string());

        let checksum = hashing::sha256_base64(part);
        let mut extra = BTreeMap::new();
        extra.insert("x-amz-checksum-sha256".to_string(), checksum.clone());

        let resp = self.execute("PUT", &uri, &query, extra, part)?;
        let etag = resp
            .header("ETag")
            .unwrap_or_default()
            .trim_matches('"')
            .to_string();
        let returned_checksum = resp.header("x-amz-checksum-sha256").map(str::to_string);
        if returned_checksum.as_deref() != Some(checksum.as_str()) {
            return Err(AppError::S3(format!(
                "part {part_number} of {key} failed checksum verification: sent {checksum}, S3 returned {returned_checksum:?}"
            )));
        }

        Ok(xml::CompletedPart {
            part_number,
            etag,
            checksum_sha256: checksum,
        })
    }

    /// Finishes a multipart upload, then verifies the composite SHA-256
    /// checksum S3 reports against the same value computed independently
    /// from the already-confirmed per-part digests. This is what actually
    /// proves the assembled object matches what was sent: a correct
    /// per-part checksum alone doesn't catch parts being combined out of
    /// order or a part silently dropped during assembly.
    fn complete_multipart_upload(
        &self,
        key: &str,
        upload_id: &str,
        parts: &[xml::CompletedPart],
    ) -> Result<PutResult, AppError> {
        let uri = self.object_uri(key);
        let mut query = BTreeMap::new();
        query.insert("uploadId".to_string(), upload_id.to_string());

        let mut extra = BTreeMap::new();
        extra.insert("content-type".to_string(), "application/xml".to_string());

        let body = xml::build_complete_multipart_upload_body(parts).into_bytes();
        let resp = self.execute("POST", &uri, &query, extra, &body)?;
        let resp_body = resp.into_string().map_err(|e| {
            AppError::S3(format!("reading complete-multipart-upload response: {e}"))
        })?;
        let parsed = xml::parse_complete_multipart_upload(&resp_body)?;

        let expected_composite = composite_sha256_checksum(parts);
        if parsed.checksum_sha256.as_deref() != Some(expected_composite.as_str()) {
            return Err(AppError::S3(format!(
                "multipart upload verification failed for {key}: expected composite SHA-256 {expected_composite}, got {:?}",
                parsed.checksum_sha256
            )));
        }

        Ok(PutResult {
            etag: parsed.etag,
            checksum_sha256: parsed.checksum_sha256,
            verified: true,
        })
    }

    /// Best-effort cancellation of an in-progress multipart upload so its
    /// parts don't linger and bill for storage. Called only as cleanup on
    /// failure; the caller doesn't (and shouldn't) treat this call's own
    /// outcome as fatal.
    fn abort_multipart_upload(&self, key: &str, upload_id: &str) -> Result<(), AppError> {
        let uri = self.object_uri(key);
        let mut query = BTreeMap::new();
        query.insert("uploadId".to_string(), upload_id.to_string());
        self.execute("DELETE", &uri, &query, BTreeMap::new(), &[])?;
        Ok(())
    }

    pub fn get_object(&self, key: &str) -> Result<Vec<u8>, AppError> {
        let uri = self.object_uri(key);
        let resp = self.execute("GET", &uri, &BTreeMap::new(), BTreeMap::new(), &[])?;
        let mut buf = Vec::new();
        resp.into_reader()
            .read_to_end(&mut buf)
            .map_err(|e| AppError::S3(format!("reading response body for {key}: {e}")))?;
        Ok(buf)
    }

    /// Downloads `key`, transparently switching from a single GET to ranged,
    /// chunked GETs once the object exceeds `threshold` bytes (split into
    /// `part_size`-byte ranges) -- the download-side mirror of
    /// `upload_object`'s multipart path, reusing the same threshold/part-size/
    /// retry config fields since the underlying motivation is identical.
    ///
    /// A single GET held open for a multi-gigabyte object (a large media
    /// folder, for instance) has to survive without a stall for as long as
    /// the whole transfer takes; the longer a connection stays open, the
    /// more likely *something* on the path (a flaky wifi link, a firewall
    /// idling out a long-lived connection, a transient AWS-side hiccup)
    /// interrupts it, and a plain GET has no way to resume or retry
    /// partway through -- the whole object has to restart from byte zero.
    /// Chunking makes each request small enough to reliably finish and
    /// cheap enough to retry on its own: with `with_retry`'s backoff on each
    /// range, one interrupted chunk costs a retry of that chunk, not the
    /// whole download.
    ///
    /// Every request goes through the same connection-pooling-disabled
    /// agent `S3Client::new` builds, so this also gets the "fresh TCP+TLS
    /// connection per request" mitigation already in place for uploads.
    pub fn download_object(
        &self,
        key: &str,
        threshold: usize,
        part_size: usize,
        part_retry_attempts: u32,
    ) -> Result<Vec<u8>, AppError> {
        let max_attempts = part_retry_attempts.saturating_add(1);

        // HEAD first so the size is known up front: it decides single-shot
        // vs. chunked, and the final assembled length is checked against it
        // below as a cheap sanity check (decryption's AEAD tag would catch a
        // truncated/corrupt result regardless, but failing here gives a much
        // clearer error message than an opaque crypto failure downstream).
        let size = self.head_object(key)?.ok_or(AppError::S3NotFound)?.size;

        if (size as usize) <= threshold {
            return with_retry(max_attempts, &format!("download of {key}"), || self.get_object(key));
        }

        let part_size = part_size.max(1);
        info(format!(
            "{key} is {size} bytes (over the {threshold}-byte threshold) -- downloading in ~{part_size}-byte ranged chunks"
        ));

        let mut buf = Vec::with_capacity(size as usize);
        for (start, end) in chunk_ranges(size, part_size as u64) {
            let chunk = with_retry(
                max_attempts,
                &format!("download of bytes {start}-{end} of {key}"),
                || self.get_object_range(key, start, end),
            )?;
            buf.extend_from_slice(&chunk);
        }

        if buf.len() as u64 != size {
            return Err(AppError::S3(format!(
                "downloaded {} bytes for {key}, expected {size} (from HEAD's Content-Length) -- \
                 the object may have changed mid-download",
                buf.len()
            )));
        }

        Ok(buf)
    }

    /// Fetches the inclusive byte range `[start, end]` of `key` via an HTTP
    /// `Range` request. Used only by `download_object`'s chunked path.
    fn get_object_range(&self, key: &str, start: u64, end: u64) -> Result<Vec<u8>, AppError> {
        let uri = self.object_uri(key);
        let mut extra = BTreeMap::new();
        extra.insert("range".to_string(), format!("bytes={start}-{end}"));
        let resp = self.execute("GET", &uri, &BTreeMap::new(), extra, &[])?;
        let mut buf = Vec::new();
        resp.into_reader().read_to_end(&mut buf).map_err(|e| {
            AppError::S3(format!(
                "reading response body for {key} (range {start}-{end}): {e}"
            ))
        })?;
        Ok(buf)
    }

    /// Returns `Ok(None)` on a 404 (object does not exist yet -- this is the
    /// normal "never backed up before" case, not an error).
    pub fn head_object(&self, key: &str) -> Result<Option<ObjectMetadata>, AppError> {
        let uri = self.object_uri(key);
        match self.execute("HEAD", &uri, &BTreeMap::new(), BTreeMap::new(), &[]) {
            Ok(resp) => {
                let size = resp
                    .header("Content-Length")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                Ok(Some(ObjectMetadata {
                    source_hash: resp.header("x-amz-meta-source-hash").map(str::to_string),
                    source_path: resp.header("x-amz-meta-source-path").map(str::to_string),
                    hostname: resp.header("x-amz-meta-hostname").map(str::to_string),
                    username: resp.header("x-amz-meta-username").map(str::to_string),
                    backup_time: resp.header("x-amz-meta-backup-time").map(str::to_string),
                    size,
                }))
            }
            Err(AppError::S3NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Lists every object under `prefix` (empty prefix = whole bucket),
    /// transparently following `NextContinuationToken` pagination.
    pub fn list_objects_v2(&self, prefix: Option<&str>) -> Result<Vec<ObjectSummary>, AppError> {
        let mut all = Vec::new();
        let mut continuation: Option<String> = None;
        let list_uri = if self.path_style {
            format!("/{}", sigv4::uri_encode_path(&self.bucket))
        } else {
            "/".to_string()
        };

        loop {
            let mut query = BTreeMap::new();
            query.insert("list-type".to_string(), "2".to_string());
            if let Some(p) = prefix {
                query.insert("prefix".to_string(), p.to_string());
            }
            if let Some(tok) = &continuation {
                query.insert("continuation-token".to_string(), tok.clone());
            }

            let resp = self.execute("GET", &list_uri, &query, BTreeMap::new(), &[])?;
            let body = resp
                .into_string()
                .map_err(|e| AppError::S3(format!("reading list-objects response: {e}")))?;
            let parsed = xml::parse_list_objects_v2(&body)?;
            all.extend(parsed.objects);

            if parsed.is_truncated {
                continuation = parsed.next_continuation_token;
                if continuation.is_none() {
                    break; // truncated but no token given back -- stop rather than loop forever
                }
            } else {
                break;
            }
        }

        Ok(all)
    }
}

/// Retries `op` up to `max_attempts` times (at least 1), with exponential
/// backoff between attempts (2s, 4s, 8s, 16s, 32s, then capped at 32s),
/// returning the first success or the last error if every attempt fails.
///
/// Used for every network call in the multipart upload path. The
/// motivating failure: a ~10 GB upload split into ~1,250 parts still hit a
/// transport-level connection abort around part 700, and because nothing
/// retried at the part level, that one failed request discarded all 700
/// already-uploaded parts and forced the whole folder to restart from a
/// fresh re-archive/re-encrypt. With even a small per-request failure
/// probability, a sequence of a thousand-plus requests is likely to hit
/// *something* eventually -- so requests need their own retry budget, not
/// just the whole file.
fn with_retry<T>(
    max_attempts: u32,
    description: &str,
    mut op: impl FnMut() -> Result<T, AppError>,
) -> Result<T, AppError> {
    let attempts = max_attempts.max(1);
    let mut last_err = None;

    for attempt in 1..=attempts {
        match op() {
            Ok(v) => return Ok(v),
            Err(e) => {
                if attempt < attempts {
                    let backoff = Duration::from_secs(2u64.saturating_pow(attempt.min(5)));
                    warn(format!(
                        "{description} failed (attempt {attempt}/{attempts}): {e} -- retrying in {}s",
                        backoff.as_secs()
                    ));
                    std::thread::sleep(backoff);
                }
                last_err = Some(e);
            }
        }
    }

    Err(last_err.expect("loop runs at least once, so last_err is always set on the error path"))
}

/// Independently recomputes the composite checksum S3 reports for a
/// completed multipart upload: SHA-256 of the concatenation of each part's
/// raw (not base64) SHA-256 digest, in part-number order, base64-encoded,
/// with `-<part_count>` appended -- the format S3's `ChecksumSHA256` takes
/// for any multipart object, mirroring the long-standing ETag `-<part_count>`
/// convention. Verified against AWS's own documented worked example in
/// `mod.rs` tests below.
/// Splits `size` bytes into inclusive `(start, end)` byte ranges of at most
/// `part_size` bytes each, for `download_object`'s chunked GET path. Pure
/// and network-free so the chunking math is directly unit-testable without
/// a mock server.
fn chunk_ranges(size: u64, part_size: u64) -> Vec<(u64, u64)> {
    if size == 0 {
        return Vec::new();
    }
    let part_size = part_size.max(1);
    let mut ranges = Vec::new();
    let mut offset = 0u64;
    while offset < size {
        let end = (offset + part_size - 1).min(size - 1);
        ranges.push((offset, end));
        offset = end + 1;
    }
    ranges
}

fn composite_sha256_checksum(parts: &[xml::CompletedPart]) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    for p in parts {
        if let Ok(raw) = STANDARD.decode(&p.checksum_sha256) {
            hasher.update(&raw);
        }
    }
    format!("{}-{}", STANDARD.encode(hasher.finalize()), parts.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composite_checksum_matches_aws_documented_example() {
        // From AWS's "Tutorial: Upload an object through multipart upload
        // and verify its data integrity" (Step 9): three part-level SHA-256
        // checksums whose decoded-and-concatenated-then-rehashed value is
        // documented to equal "aI8EoktCdotjU8Bq46DrPCxQCGuGcPIhJ51noWs6hvk=",
        // matching the ChecksumSHA256 (before the "-3" suffix) that
        // CompleteMultipartUpload returned for that same upload.
        let parts = vec![
            xml::CompletedPart {
                part_number: 1,
                etag: "irrelevant-for-this-check".to_string(),
                checksum_sha256: "QLl8R4i4+SaJlrl8ZIcutc5TbZtwt2NwB8lTXkd3GH0=".to_string(),
            },
            xml::CompletedPart {
                part_number: 2,
                etag: "irrelevant-for-this-check".to_string(),
                checksum_sha256: "xCdgs1K5Bm4jWETYw/CmGYr+m6O2DcGfpckx5NVokvE=".to_string(),
            },
            xml::CompletedPart {
                part_number: 3,
                etag: "irrelevant-for-this-check".to_string(),
                checksum_sha256: "f5wsfsa5bB+yXuwzqG1Bst91uYneqGD3CCidpb54mAo=".to_string(),
            },
        ];

        assert_eq!(
            composite_sha256_checksum(&parts),
            "aI8EoktCdotjU8Bq46DrPCxQCGuGcPIhJ51noWs6hvk=-3"
        );
    }

    #[test]
    fn chunk_ranges_splits_into_part_sized_pieces() {
        assert_eq!(chunk_ranges(10, 3), vec![(0, 2), (3, 5), (6, 8), (9, 9)]);
    }

    #[test]
    fn chunk_ranges_exact_multiple_of_part_size() {
        assert_eq!(chunk_ranges(9, 3), vec![(0, 2), (3, 5), (6, 8)]);
    }

    #[test]
    fn chunk_ranges_single_range_when_object_smaller_than_part_size() {
        assert_eq!(chunk_ranges(5, 100), vec![(0, 4)]);
    }

    #[test]
    fn chunk_ranges_empty_object_has_no_ranges() {
        assert_eq!(chunk_ranges(0, 100), Vec::<(u64, u64)>::new());
    }

    #[test]
    fn chunk_ranges_zero_part_size_treated_as_one() {
        // Defensive: a misconfigured part size of 0 must not loop forever or
        // divide by zero -- it degrades to one byte per range instead.
        assert_eq!(chunk_ranges(2, 0), vec![(0, 0), (1, 1)]);
    }
}

fn map_ureq_error(err: ureq::Error) -> AppError {
    match err {
        ureq::Error::Status(404, _) => AppError::S3NotFound,
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            let message = xml::extract_error_message(&body);
            AppError::S3(format!("HTTP {code}: {message}"))
        }
        ureq::Error::Transport(t) => AppError::S3(format!("transport error: {t}")),
    }
}
