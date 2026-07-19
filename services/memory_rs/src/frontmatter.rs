//! Parsing and rendering of the canonical `---`-delimited YAML frontmatter
//! plus a Markdown body.

use chrono::{DateTime, Utc};
use serde_yaml_ng::{Mapping, Value};

use crate::{error::MemoryError, model::PageMetadata};

const FRONTMATTER_DELIMITER: &str = "---";

/// Parses a full page document (frontmatter + body) from its exact stored
/// bytes.
///
/// # Errors
///
/// Returns [`MemoryError::InvalidFrontmatter`] if the bytes are not UTF-8,
/// the frontmatter delimiters are missing, the YAML is malformed, or a
/// required field is missing or has the wrong type.
pub fn parse_document(
    bytes: &[u8],
    path_hint: &str,
) -> Result<(PageMetadata, String), MemoryError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_error| MemoryError::invalid_frontmatter(path_hint, "content must be UTF-8"))?;

    let after_open = strip_opening_delimiter(text).ok_or_else(|| {
        MemoryError::invalid_frontmatter(path_hint, "missing opening '---' frontmatter delimiter")
    })?;

    let (yaml_text, body) = split_frontmatter(after_open).ok_or_else(|| {
        MemoryError::invalid_frontmatter(path_hint, "missing closing '---' frontmatter delimiter")
    })?;

    let metadata = parse_metadata(yaml_text, path_hint)?;
    Ok((metadata, body.to_owned()))
}

fn strip_opening_delimiter(text: &str) -> Option<&str> {
    text.strip_prefix("---\r\n")
        .or_else(|| text.strip_prefix("---\n"))
}

/// Splits the text after the opening delimiter into the raw YAML block and
/// the body. The closing delimiter line's own line terminator is always
/// consumed; if it is immediately followed by a single blank separator line
/// (as produced by [`render_document`]), that blank line is consumed too and
/// does not appear in the returned body.
fn split_frontmatter(after_open: &str) -> Option<(&str, &str)> {
    let mut search_start = 0;
    loop {
        let relative = after_open[search_start..].find("\n---")?;
        let marker_index = search_start + relative;
        let rest = &after_open[marker_index + 4..];
        let Some(after_delimiter_line) = rest
            .strip_prefix("\r\n")
            .or_else(|| rest.strip_prefix('\n'))
        else {
            if rest.is_empty() {
                return Some((&after_open[..marker_index], ""));
            }
            // The matched "---" was not actually a delimiter line (not
            // followed by end-of-line); keep searching further in the text.
            search_start = marker_index + 4;
            continue;
        };
        let body = after_delimiter_line
            .strip_prefix("\r\n")
            .or_else(|| after_delimiter_line.strip_prefix('\n'))
            .unwrap_or(after_delimiter_line);
        return Some((&after_open[..marker_index], body));
    }
}

fn parse_metadata(yaml_text: &str, path_hint: &str) -> Result<PageMetadata, MemoryError> {
    let value: Value = serde_yaml_ng::from_str(yaml_text)?;
    let Value::Mapping(mut mapping) = value else {
        return Err(MemoryError::invalid_frontmatter(
            path_hint,
            "frontmatter must be a YAML mapping",
        ));
    };

    let schema_version = take_u64(&mut mapping, "schema_version").ok_or_else(|| {
        MemoryError::invalid_frontmatter(path_hint, "missing required field 'schema_version'")
    })?;
    let title = take_string(&mut mapping, "title").ok_or_else(|| {
        MemoryError::invalid_frontmatter(path_hint, "missing required field 'title'")
    })?;
    let tags = take_string_sequence(&mut mapping, "tags").ok_or_else(|| {
        MemoryError::invalid_frontmatter(path_hint, "missing required field 'tags'")
    })?;
    let created_at = take_datetime(&mut mapping, "created_at", path_hint)?.ok_or_else(|| {
        MemoryError::invalid_frontmatter(path_hint, "missing required field 'created_at'")
    })?;
    let updated_at = take_datetime(&mut mapping, "updated_at", path_hint)?.ok_or_else(|| {
        MemoryError::invalid_frontmatter(path_hint, "missing required field 'updated_at'")
    })?;
    let created_by = take_string(&mut mapping, "created_by");
    let updated_by = take_string(&mut mapping, "updated_by");

    Ok(PageMetadata {
        schema_version,
        title,
        tags,
        created_at,
        updated_at,
        created_by,
        updated_by,
        extra: mapping,
    })
}

fn take_u64(mapping: &mut Mapping, key: &str) -> Option<u64> {
    mapping
        .remove(Value::String(key.to_owned()))
        .and_then(|value| value.as_u64())
}

fn take_string(mapping: &mut Mapping, key: &str) -> Option<String> {
    match mapping.remove(Value::String(key.to_owned())) {
        Some(Value::String(value)) => Some(value),
        _ => None,
    }
}

fn take_string_sequence(mapping: &mut Mapping, key: &str) -> Option<Vec<String>> {
    match mapping.remove(Value::String(key.to_owned())) {
        Some(Value::Sequence(items)) => items
            .into_iter()
            .map(|item| match item {
                Value::String(value) => Some(value),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

fn take_datetime(
    mapping: &mut Mapping,
    key: &str,
    path_hint: &str,
) -> Result<Option<DateTime<Utc>>, MemoryError> {
    let Some(value) = mapping.remove(Value::String(key.to_owned())) else {
        return Ok(None);
    };
    let raw = match &value {
        Value::String(raw) => raw.clone(),
        other => serde_yaml_ng::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_owned(),
    };
    let parsed = DateTime::parse_from_rfc3339(&raw).map_err(|error| {
        MemoryError::invalid_frontmatter(
            path_hint,
            format!("field {key:?} is not RFC 3339: {error}"),
        )
    })?;
    Ok(Some(parsed.with_timezone(&Utc)))
}

/// Canonicalizes a page body to exactly one trailing newline (or empty).
///
/// Matches the form [`render_document`] stores on disk. Used so in-memory
/// results (e.g. the [`crate::model::Page`] returned from a write) reflect
/// the same bytes that were persisted.
#[must_use]
pub fn normalize_body(body: &str) -> String {
    let trimmed = body.trim_end_matches('\n');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}\n")
    }
}

/// Renders a page's exact stored bytes from its metadata and body.
///
/// The output is the canonical form used to compute a page's [`crate::model::Revision`]:
/// a `---`-delimited YAML block with fields in a fixed order followed by
/// preserved unknown fields, then a blank line, then the body with exactly
/// one trailing newline (or no trailing content if the body is empty).
///
/// # Errors
///
/// Returns [`MemoryError::Yaml`] if the frontmatter cannot be serialized.
pub fn render_document(metadata: &PageMetadata, body: &str) -> Result<Vec<u8>, MemoryError> {
    let mut mapping = Mapping::new();
    mapping.insert(
        Value::String("schema_version".to_owned()),
        Value::from(metadata.schema_version),
    );
    mapping.insert(
        Value::String("title".to_owned()),
        Value::String(metadata.title.clone()),
    );
    mapping.insert(
        Value::String("tags".to_owned()),
        Value::Sequence(metadata.tags.iter().cloned().map(Value::String).collect()),
    );
    mapping.insert(
        Value::String("created_at".to_owned()),
        Value::String(metadata.created_at.to_rfc3339()),
    );
    mapping.insert(
        Value::String("updated_at".to_owned()),
        Value::String(metadata.updated_at.to_rfc3339()),
    );
    if let Some(created_by) = &metadata.created_by {
        mapping.insert(
            Value::String("created_by".to_owned()),
            Value::String(created_by.clone()),
        );
    }
    if let Some(updated_by) = &metadata.updated_by {
        mapping.insert(
            Value::String("updated_by".to_owned()),
            Value::String(updated_by.clone()),
        );
    }
    for (key, value) in &metadata.extra {
        mapping.insert(key.clone(), value.clone());
    }

    let yaml = serde_yaml_ng::to_string(&Value::Mapping(mapping))?;
    let normalized_body = normalize_body(body);

    let mut document = String::new();
    document.push_str(FRONTMATTER_DELIMITER);
    document.push('\n');
    document.push_str(&yaml);
    document.push_str(FRONTMATTER_DELIMITER);
    document.push('\n');
    if !normalized_body.is_empty() {
        document.push('\n');
        document.push_str(&normalized_body);
    }

    Ok(document.into_bytes())
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_yaml_ng::Mapping;

    use super::{parse_document, render_document};
    use crate::model::PageMetadata;

    fn sample_metadata() -> PageMetadata {
        PageMetadata {
            schema_version: 1,
            title: "Alice".to_owned(),
            tags: vec!["birthday".to_owned(), "person".to_owned()],
            created_at: chrono::Utc.with_ymd_and_hms(2026, 7, 17, 6, 35, 9).unwrap(),
            updated_at: chrono::Utc.with_ymd_and_hms(2026, 7, 17, 6, 35, 9).unwrap(),
            created_by: Some("research-agent".to_owned()),
            updated_by: Some("research-agent".to_owned()),
            extra: Mapping::new(),
        }
    }

    #[test]
    fn round_trips_document() {
        let metadata = sample_metadata();
        let bytes = render_document(&metadata, "Alice's birthday is ...\n")
            .unwrap_or_else(|error| panic!("render: {error}"));
        let (parsed, body) =
            parse_document(&bytes, "people/alice").unwrap_or_else(|error| panic!("parse: {error}"));
        assert_eq!(parsed, metadata);
        assert_eq!(body, "Alice's birthday is ...\n");
    }

    #[test]
    fn preserves_unknown_fields() {
        let mut metadata = sample_metadata();
        metadata
            .extra
            .insert("source".into(), "manual-import".into());
        let bytes =
            render_document(&metadata, "body").unwrap_or_else(|error| panic!("render: {error}"));
        let (parsed, _) =
            parse_document(&bytes, "people/alice").unwrap_or_else(|error| panic!("parse: {error}"));
        assert_eq!(
            parsed.extra.get("source").and_then(|v| v.as_str()),
            Some("manual-import")
        );
    }

    #[test]
    fn rejects_missing_delimiter() {
        parse_document(b"no frontmatter here", "x")
            .map_or_else(|error| error, |_| panic!("must reject"));
    }

    #[test]
    fn rejects_missing_required_field() {
        let bytes = b"---\ntitle: Alice\n---\nbody\n";
        parse_document(bytes, "x").map_or_else(
            |error| error,
            |_| panic!("must reject missing schema_version"),
        );
    }

    #[test]
    fn empty_body_produces_no_trailing_blank_section() {
        let metadata = sample_metadata();
        let bytes =
            render_document(&metadata, "").unwrap_or_else(|error| panic!("render: {error}"));
        let text = String::from_utf8(bytes).unwrap_or_else(|error| panic!("utf8: {error}"));
        assert!(text.ends_with("---\n"));
    }
}
