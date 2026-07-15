//! Object naming convention, kept compatible with the original
//! `getArchiveName()`: `<hostname>_<username>_<sanitized_folder_path>`, with
//! path separators, colons, and spaces replaced by underscores -- this is
//! what makes the "multi-host awareness" requirement work (see requirements
//! doc section 9): distinct hosts/users backing up the same-named folder
//! into a shared bucket don't collide.
//!
//! The result doubles as a local filename (backup.rs builds temp file paths
//! from it), so it has to be valid on every platform we run on, not just as
//! an S3 key. S3 keys tolerate almost anything; Windows filenames don't --
//! `< > : " / \ | ? *` and control characters are all rejected there, so
//! all of them are sanitized here rather than just the separator characters
//! a Unix-only version would need.

pub const ARCHIVE_SUFFIX: &str = ".tar.gz.enc";

pub fn sanitize_path_component(path: &str) -> String {
    let mapped: String = path
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | ' ' | '<' | '>' | '"' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            other => other,
        })
        .collect();

    // Collapse runs of underscores (e.g. from "C:\" -> ':' and '\' both
    // mapping to '_' back to back) so names stay readable rather than
    // accumulating double/triple underscores at every separator boundary.
    let mut out = String::with_capacity(mapped.len());
    let mut prev_was_underscore = false;
    for c in mapped.chars() {
        if c == '_' {
            if !prev_was_underscore {
                out.push(c);
            }
            prev_was_underscore = true;
        } else {
            out.push(c);
            prev_was_underscore = false;
        }
    }
    out.trim_matches('_').to_string()
}

/// Builds the S3 object key for a given folder under a given backup root.
pub fn object_key(hostname: &str, username: &str, folder_path: &str) -> String {
    format!(
        "{}_{}_{}{}",
        sanitize_path_component(hostname),
        sanitize_path_component(username),
        sanitize_path_component(folder_path),
        ARCHIVE_SUFFIX
    )
}

/// Strips the `.enc`, `.gz`, `.tar` suffixes (in that order) from an object
/// name to recover its "base" name, matching the original restore logic in
/// the requirements doc (section 5.3). Falls back to the full name if the
/// expected suffix chain isn't present, rather than panicking, since restore
/// may encounter objects that don't follow this convention (e.g. a manifest
/// or audit-log object) -- those should simply be skipped by the caller.
pub fn base_name_from_object_key(key: &str) -> Option<String> {
    key.strip_suffix(ARCHIVE_SUFFIX).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_path_separators_and_spaces() {
        assert_eq!(
            sanitize_path_component("/Users/eric/My Documents"),
            "Users_eric_My_Documents"
        );
    }

    #[test]
    fn windows_path_sanitized() {
        assert_eq!(sanitize_path_component(r"C:\Users\eric"), "C_Users_eric");
    }

    #[test]
    fn windows_reserved_filename_characters_sanitized() {
        // < > : " / \ | ? * are all rejected in Windows filenames; the
        // sanitized result is used as a local temp filename, not just an S3
        // key, so all of them need to come out clean.
        assert_eq!(sanitize_path_component("a<b>c:d\"e|f?g*h"), "a_b_c_d_e_f_g_h");
    }

    #[test]
    fn object_key_round_trips_to_base_name() {
        let key = object_key("erics-mbp", "eric", "/Users/eric/Documents");
        assert_eq!(key, "erics-mbp_eric_Users_eric_Documents.tar.gz.enc");
        assert_eq!(
            base_name_from_object_key(&key).as_deref(),
            Some("erics-mbp_eric_Users_eric_Documents")
        );
    }

    #[test]
    fn non_matching_object_returns_none() {
        assert_eq!(base_name_from_object_key("_s3b/manifest.json"), None);
    }
}
