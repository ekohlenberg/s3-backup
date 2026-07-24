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

        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout(Duration::from_secs(300))
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
    pub fn upload_object(
        &self,
        key: &str,
        body: &[u8],
        metadata: &[(&str, &str)],
        threshold: usize,
        part_size: usize,
    ) -> Result<PutResult, AppError> {
        if body.len() <= threshold {
            return self.put_object(key, body, metadata);
        }

        let part_size = part_size.max(MIN_MULTIPART_PART_SIZE);
        let upload_id = self.create_multipart_upload(key, metadata)?;

        match self.upload_parts_and_complete(key, &upload_id, body, part_size) {
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
    ) -> Result<PutResult, AppError> {
        let mut parts = Vec::new();
        for (i, chunk) in body.chunks(part_size).enumerate() {
            let part_number = (i + 1) as u32;
            parts.push(self.upload_part(key, upload_id, part_number, chunk)?);
        }
        self.complete_multipart_upload(key, upload_id, &parts)
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

/// Independently recomputes the composite checksum S3 reports for a
/// completed multipart upload: SHA-256 of the concatenation of each part's
/// raw (not base64) SHA-256 digest, in part-number order, base64-encoded,
/// with `-<part_count>` appended -- the format S3's `ChecksumSHA256` takes
/// for any multipart object, mirroring the long-standing ETag `-<part_count>`
/// convention. Verified against AWS's own documented worked example in
/// `mod.rs` tests below.
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
