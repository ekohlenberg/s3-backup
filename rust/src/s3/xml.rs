//! Minimal, purpose-built XML parsing for S3's `ListObjectsV2` response
//! (and error responses), using `quick-xml`'s low-level event API rather
//! than a full serde-XML-derive stack -- we only ever need five fields.

use quick_xml::events::Event;
use quick_xml::reader::Reader;

use crate::error::AppError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectSummary {
    pub key: String,
    pub size: u64,
    pub etag: String, // quotes stripped
}

#[derive(Debug, Clone, Default)]
pub struct ListObjectsV2Result {
    pub objects: Vec<ObjectSummary>,
    pub is_truncated: bool,
    pub next_continuation_token: Option<String>,
}

pub fn parse_list_objects_v2(xml: &str) -> Result<ListObjectsV2Result, AppError> {
    // Note: text-node trimming is deliberately not enabled here (the exact
    // config API for this has changed across quick-xml versions). It isn't
    // needed for correctness: whitespace-only text nodes between tags are
    // only ever seen while `current_tag` is a container tag name (e.g.
    // `Contents`), which the match arms below simply ignore.
    let mut reader = Reader::from_str(xml);

    let mut result = ListObjectsV2Result::default();
    let mut in_contents = false;
    let mut current_tag = String::new();
    let mut cur_key = String::new();
    let mut cur_size: u64 = 0;
    let mut cur_etag = String::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref());
                if name == "Contents" {
                    in_contents = true;
                    cur_key.clear();
                    cur_size = 0;
                    cur_etag.clear();
                }
                current_tag = name;
            }
            Ok(Event::Text(e)) => {
                let text = e
                    .unescape()
                    .map_err(|err| AppError::S3(format!("invalid XML text: {err}")))?
                    .into_owned();
                if in_contents {
                    match current_tag.as_str() {
                        "Key" => cur_key = text,
                        "Size" => cur_size = text.parse().unwrap_or(0),
                        "ETag" => cur_etag = text.trim_matches('"').to_string(),
                        _ => {}
                    }
                } else {
                    match current_tag.as_str() {
                        "IsTruncated" => result.is_truncated = text == "true",
                        "NextContinuationToken" => result.next_continuation_token = Some(text),
                        _ => {}
                    }
                }
            }
            Ok(Event::End(e)) => {
                let name = local_name(e.name().as_ref());
                if name == "Contents" {
                    in_contents = false;
                    result.objects.push(ObjectSummary {
                        key: cur_key.clone(),
                        size: cur_size,
                        etag: cur_etag.clone(),
                    });
                }
                // Without this, a whitespace-only text node between this
                // closing tag and the next sibling's opening tag would still
                // see `current_tag` as the tag that just closed (Start is the
                // only other place that sets it), and get misread as content
                // for that field -- e.g. the "\n  " between `</IsTruncated>`
                // and `<Contents>` was overwriting `is_truncated` back to
                // false right after it had been read correctly.
                current_tag.clear();
            }
            Err(err) => return Err(AppError::S3(format!("malformed XML response: {err}"))),
            _ => {}
        }
        buf.clear();
    }

    Ok(result)
}

fn local_name(qualified: &[u8]) -> String {
    let s = String::from_utf8_lossy(qualified);
    s.rsplit(':').next().unwrap_or(&s).to_string()
}

/// One part's identifying info needed to build a `CompleteMultipartUpload`
/// request body: the part number, the ETag S3 returned for that part's
/// `UploadPart` call, and the base64 SHA-256 checksum computed locally for
/// that part. Sending the checksum alongside the ETag makes S3 re-validate
/// every part's checksum again at completion time, not just when the part
/// was first uploaded.
#[derive(Debug, Clone)]
pub struct CompletedPart {
    pub part_number: u32,
    pub etag: String,
    pub checksum_sha256: String,
}

/// Extracts `<UploadId>` from a `CreateMultipartUpload` response body.
pub fn parse_upload_id(xml: &str) -> Result<String, AppError> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut current_tag = String::new();
    let mut upload_id = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => current_tag = local_name(e.name().as_ref()),
            Ok(Event::Text(e)) => {
                if current_tag == "UploadId" {
                    if let Ok(text) = e.unescape() {
                        upload_id = Some(text.into_owned());
                    }
                }
            }
            Ok(Event::End(_)) => current_tag.clear(),
            Err(err) => return Err(AppError::S3(format!("malformed XML response: {err}"))),
            _ => {}
        }
        buf.clear();
    }

    upload_id.ok_or_else(|| {
        AppError::S3(format!(
            "CreateMultipartUpload response missing UploadId: {}",
            extract_error_message(xml)
        ))
    })
}

/// Builds the `CompleteMultipartUpload` request body: an ordered list of
/// parts. Part numbers must appear in ascending order for S3 to accept the
/// request.
pub fn build_complete_multipart_upload_body(parts: &[CompletedPart]) -> String {
    let mut body = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?><CompleteMultipartUpload xmlns="http://s3.amazonaws.com/doc/2006-03-01/">"#,
    );
    for p in parts {
        body.push_str(&format!(
            "<Part><PartNumber>{}</PartNumber><ETag>\"{}\"</ETag><ChecksumSHA256>{}</ChecksumSHA256></Part>",
            p.part_number, p.etag, p.checksum_sha256
        ));
    }
    body.push_str("</CompleteMultipartUpload>");
    body
}

#[derive(Debug, Clone, Default)]
pub struct CompleteMultipartUploadResponse {
    pub etag: String,
    pub checksum_sha256: Option<String>,
}

/// Parses a `CompleteMultipartUpload` response body. S3 has a documented
/// quirk here: it can return HTTP 200 with headers sent before the request
/// actually finished, then report failure via an `<Error>` element in the
/// body instead of an HTTP error status -- so an empty/missing `ETag` is
/// treated as a failure (with whatever message the body carries) rather
/// than silently returning a blank success.
pub fn parse_complete_multipart_upload(
    xml: &str,
) -> Result<CompleteMultipartUploadResponse, AppError> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut current_tag = String::new();
    let mut result = CompleteMultipartUploadResponse::default();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => current_tag = local_name(e.name().as_ref()),
            Ok(Event::Text(e)) => {
                let text = e.unescape().map(|c| c.into_owned()).unwrap_or_default();
                match current_tag.as_str() {
                    "ETag" => result.etag = text.trim_matches('"').to_string(),
                    "ChecksumSHA256" => result.checksum_sha256 = Some(text),
                    _ => {}
                }
            }
            Ok(Event::End(_)) => current_tag.clear(),
            Err(err) => return Err(AppError::S3(format!("malformed XML response: {err}"))),
            _ => {}
        }
        buf.clear();
    }

    if result.etag.is_empty() {
        return Err(AppError::S3(format!(
            "CompleteMultipartUpload failed: {}",
            extract_error_message(xml)
        )));
    }

    Ok(result)
}

/// Extracts `<Message>` from an S3 XML error body, falling back to the raw
/// body if it doesn't parse as expected -- used to surface a useful error
/// string instead of just an HTTP status code.
pub fn extract_error_message(xml: &str) -> String {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut current_tag = String::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => current_tag = local_name(e.name().as_ref()),
            Ok(Event::Text(e)) => {
                if current_tag == "Message" {
                    if let Ok(text) = e.unescape() {
                        return text.into_owned();
                    }
                }
            }
            Ok(Event::End(_)) => current_tag.clear(),
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    xml.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_contents_and_truncation() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
  <Name>examplebucket</Name>
  <Prefix></Prefix>
  <KeyCount>2</KeyCount>
  <MaxKeys>1000</MaxKeys>
  <IsTruncated>true</IsTruncated>
  <Contents>
    <Key>host_user_folder-a.tar.gz.enc</Key>
    <LastModified>2026-07-13T18:42:00.000Z</LastModified>
    <ETag>"d41d8cd98f00b204e9800998ecf8427e"</ETag>
    <Size>1234</Size>
    <StorageClass>STANDARD</StorageClass>
  </Contents>
  <Contents>
    <Key>_s3b/manifest.json</Key>
    <LastModified>2026-07-13T18:42:00.000Z</LastModified>
    <ETag>"abc123"</ETag>
    <Size>256</Size>
    <StorageClass>STANDARD</StorageClass>
  </Contents>
  <NextContinuationToken>token-xyz</NextContinuationToken>
</ListBucketResult>"#;

        let parsed = parse_list_objects_v2(xml).unwrap();
        assert!(parsed.is_truncated);
        assert_eq!(parsed.next_continuation_token.as_deref(), Some("token-xyz"));
        assert_eq!(parsed.objects.len(), 2);
        assert_eq!(parsed.objects[0].key, "host_user_folder-a.tar.gz.enc");
        assert_eq!(parsed.objects[0].size, 1234);
        assert_eq!(parsed.objects[0].etag, "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(parsed.objects[1].key, "_s3b/manifest.json");
    }

    #[test]
    fn empty_bucket_parses_cleanly() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
  <Name>examplebucket</Name>
  <KeyCount>0</KeyCount>
  <MaxKeys>1000</MaxKeys>
  <IsTruncated>false</IsTruncated>
</ListBucketResult>"#;
        let parsed = parse_list_objects_v2(xml).unwrap();
        assert!(!parsed.is_truncated);
        assert!(parsed.objects.is_empty());
        assert!(parsed.next_continuation_token.is_none());
    }

    #[test]
    fn extracts_error_message() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<Error>
  <Code>NoSuchKey</Code>
  <Message>The specified key does not exist.</Message>
  <Key>foo</Key>
  <RequestId>abc</RequestId>
</Error>"#;
        assert_eq!(extract_error_message(xml), "The specified key does not exist.");
    }

    #[test]
    fn parses_upload_id_from_create_multipart_upload_response() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<InitiateMultipartUploadResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
  <Bucket>examplebucket</Bucket>
  <Key>census_data_file</Key>
  <UploadId>cNV6KCSNANFZapz1LUGPC5XwUVi1n6yUoIeSP138sNOKPeMhpKQRrbT9k0ePmgoOTCj9K83T4e2Gb5hQvNoNpCKqyb8m3.oyYgQNZD6FNJLBZluOIUyRE.qM5yhDTdhz</UploadId>
</InitiateMultipartUploadResult>"#;
        assert_eq!(
            parse_upload_id(xml).unwrap(),
            "cNV6KCSNANFZapz1LUGPC5XwUVi1n6yUoIeSP138sNOKPeMhpKQRrbT9k0ePmgoOTCj9K83T4e2Gb5hQvNoNpCKqyb8m3.oyYgQNZD6FNJLBZluOIUyRE.qM5yhDTdhz"
        );
    }

    #[test]
    fn missing_upload_id_is_an_error() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?><Error><Message>denied</Message></Error>"#;
        assert!(parse_upload_id(xml).is_err());
    }

    #[test]
    fn builds_complete_multipart_upload_body_in_part_order() {
        let parts = vec![
            CompletedPart {
                part_number: 1,
                etag: "e611693805e812ef37f96c9937605e69".to_string(),
                checksum_sha256: "QLl8R4i4+SaJlrl8ZIcutc5TbZtwt2NwB8lTXkd3GH0=".to_string(),
            },
            CompletedPart {
                part_number: 2,
                etag: "63d2d5da159178785bfd6b6a5c635854".to_string(),
                checksum_sha256: "xCdgs1K5Bm4jWETYw/CmGYr+m6O2DcGfpckx5NVokvE=".to_string(),
            },
        ];
        let body = build_complete_multipart_upload_body(&parts);
        assert!(body.starts_with("<?xml"));
        assert!(body.contains("<PartNumber>1</PartNumber><ETag>\"e611693805e812ef37f96c9937605e69\"</ETag><ChecksumSHA256>QLl8R4i4+SaJlrl8ZIcutc5TbZtwt2NwB8lTXkd3GH0=</ChecksumSHA256>"));
        assert!(body.contains("<PartNumber>2</PartNumber>"));
        assert!(body.ends_with("</CompleteMultipartUpload>"));
    }

    #[test]
    fn parses_complete_multipart_upload_response() {
        // From the AWS multipart-upload-with-checksums tutorial.
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<CompleteMultipartUploadResult>
  <Location>https://amzn-s3-demo-bucket1.s3.us-east-2.amazonaws.com/census_data_file</Location>
  <Bucket>amzn-s3-demo-bucket1</Bucket>
  <Key>census_data_file</Key>
  <ETag>"f453c6dccca969c457efdf9b1361e291-3"</ETag>
  <ChecksumSHA256>aI8EoktCdotjU8Bq46DrPCxQCGuGcPIhJ51noWs6hvk=-3</ChecksumSHA256>
</CompleteMultipartUploadResult>"#;
        let parsed = parse_complete_multipart_upload(xml).unwrap();
        assert_eq!(parsed.etag, "f453c6dccca969c457efdf9b1361e291-3");
        assert_eq!(
            parsed.checksum_sha256.as_deref(),
            Some("aI8EoktCdotjU8Bq46DrPCxQCGuGcPIhJ51noWs6hvk=-3")
        );
    }

    #[test]
    fn empty_etag_in_complete_multipart_upload_response_is_an_error() {
        // S3's documented quirk: HTTP 200 with an <Error> body instead of an
        // HTTP error status if something goes wrong after headers are sent.
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<Error>
  <Code>InternalError</Code>
  <Message>We encountered an internal error. Please try again.</Message>
</Error>"#;
        let err = parse_complete_multipart_upload(xml).unwrap_err();
        assert!(err.to_string().contains("We encountered an internal error"));
    }
}
