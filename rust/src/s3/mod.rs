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
    /// confirming the digest it validated the uploaded bytes against.
    pub checksum_sha256: Option<String>,
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
        })
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
