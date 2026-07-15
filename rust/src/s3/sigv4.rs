//! Hand-rolled AWS Signature Version 4 request signing, replacing the
//! `aws` CLI shell-out. Implements exactly the subset SigV4 needs for our
//! four operations (PUT, GET, HEAD, ListObjectsV2 over GET with a query
//! string) -- not a general-purpose AWS SDK.
//!
//! Reference: https://docs.aws.amazon.com/general/latest/gr/sigv4-create-canonical-request.html

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

type HmacSha256 = Hmac<Sha256>;

pub const EMPTY_PAYLOAD_SHA256: &str =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac =
        HmacSha256::new_from_slice(key).expect("HMAC-SHA256 accepts a key of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Percent-encodes a single path segment per RFC 3986 unreserved characters
/// (SigV4's `UriEncode` with `encodeSlash = false` applied segment-by-segment
/// by the caller, who re-joins with `/`).
pub fn uri_encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Encodes a full object key path (`a/b c/d.txt` -> `a/b%20c/d.txt`),
/// preserving `/` as a segment separator. This encoded value is used both
/// in the actual HTTP request URI and, unchanged, in the canonical request
/// -- S3 is the one SigV4 service where the canonical URI is *not*
/// double-encoded.
pub fn uri_encode_path(path: &str) -> String {
    path.split('/')
        .map(uri_encode_segment)
        .collect::<Vec<_>>()
        .join("/")
}

/// Encodes a query parameter key or value per SigV4 rules (same unreserved
/// set as path encoding, but `/` is also percent-encoded here since this is
/// not a path).
pub fn uri_encode_query_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

pub fn canonical_query_string(params: &BTreeMap<String, String>) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", uri_encode_query_component(k), uri_encode_query_component(v)))
        .collect::<Vec<_>>()
        .join("&")
}

pub struct SigningInput<'a> {
    pub method: &'a str,
    /// Already-encoded absolute path, e.g. `/my key.txt` -> `/my%20key.txt`.
    pub canonical_uri: &'a str,
    /// Pre-sorted, pre-encoded query string (`""` if none).
    pub canonical_query_string: &'a str,
    /// Lowercased header name -> trimmed value. Must include at least
    /// `host`, `x-amz-content-sha256`, and `x-amz-date`.
    pub headers: &'a BTreeMap<String, String>,
    pub payload_sha256_hex: &'a str,
    pub region: &'a str,
    pub access_key: &'a str,
    pub secret_key: &'a str,
    pub amz_date: &'a str,   // YYYYMMDDTHHMMSSZ
    pub date_stamp: &'a str, // YYYYMMDD
}

pub struct SignedAuth {
    pub authorization_header: String,
}

const SERVICE: &str = "s3";

pub fn sign(input: &SigningInput) -> SignedAuth {
    let signed_headers = input
        .headers
        .keys()
        .cloned()
        .collect::<Vec<_>>()
        .join(";");

    let canonical_headers = input
        .headers
        .iter()
        .map(|(k, v)| format!("{k}:{v}\n"))
        .collect::<String>();

    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        input.method,
        input.canonical_uri,
        input.canonical_query_string,
        canonical_headers,
        signed_headers,
        input.payload_sha256_hex
    );

    let credential_scope = format!(
        "{}/{}/{}/aws4_request",
        input.date_stamp, input.region, SERVICE
    );

    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{}\n{}",
        input.amz_date,
        credential_scope,
        sha256_hex(canonical_request.as_bytes())
    );

    let k_date = hmac(format!("AWS4{}", input.secret_key).as_bytes(), input.date_stamp.as_bytes());
    let k_region = hmac(&k_date, input.region.as_bytes());
    let k_service = hmac(&k_region, SERVICE.as_bytes());
    let k_signing = hmac(&k_service, b"aws4_request");
    let signature = hex::encode(hmac(&k_signing, string_to_sign.as_bytes()));

    let authorization_header = format!(
        "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
        input.access_key, credential_scope, signed_headers, signature
    );

    SignedAuth { authorization_header }
}

#[cfg(test)]
mod tests {
    use super::*;

    // AWS's published SigV4 test suite includes a canonical "get-vanilla"
    // case. We reconstruct the well-known GET example from the SigV4
    // documentation (s3 GET example.amazonaws.com, 20130524) since it gives
    // fixed, checkable intermediate values.
    #[test]
    fn matches_aws_documentation_get_example() {
        // Values from:
        // https://docs.aws.amazon.com/AmazonS3/latest/API/sig-v4-authenticating-requests.html
        // "GET Object" example.
        let mut headers = BTreeMap::new();
        headers.insert("host".to_string(), "examplebucket.s3.amazonaws.com".to_string());
        headers.insert(
            "x-amz-content-sha256".to_string(),
            EMPTY_PAYLOAD_SHA256.to_string(),
        );
        headers.insert("x-amz-date".to_string(), "20130524T000000Z".to_string());
        headers.insert("range".to_string(), "bytes=0-9".to_string());

        let input = SigningInput {
            method: "GET",
            canonical_uri: "/test.txt",
            canonical_query_string: "",
            headers: &headers,
            payload_sha256_hex: EMPTY_PAYLOAD_SHA256,
            region: "us-east-1",
            access_key: "AKIAIOSFODNN7EXAMPLE",
            secret_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            amz_date: "20130524T000000Z",
            date_stamp: "20130524",
        };

        let signed = sign(&input);
        // Cross-checked independently with a Python hashlib/hmac
        // implementation of the same canonical request / signing steps.
        assert_eq!(
            signed.authorization_header,
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, \
             SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, \
             Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    #[test]
    fn path_encoding_preserves_slashes_and_encodes_spaces() {
        assert_eq!(uri_encode_path("a/b c/d.txt"), "a/b%20c/d.txt");
    }

    #[test]
    fn query_encoding_encodes_slashes() {
        assert_eq!(uri_encode_query_component("a/b"), "a%2Fb");
    }
}
