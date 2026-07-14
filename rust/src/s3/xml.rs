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
}
