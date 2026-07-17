//! Discovery and rewriting of ordinary relative Markdown links between
//! store pages, e.g. `[Alice](../people/alice.md)`.
//!
//! This module intentionally implements the first-version scope described in
//! `MEMORY_PLAN.md`: literal `[text](target)` inline links to other `.md`
//! pages. It does not claim to preserve reference-style links (`[text][ref]`),
//! links inside arbitrary HTML, or external/absolute URLs.

use crate::path::PagePath;

/// One inline Markdown link found in a page body, with byte offsets into
/// that body so callers can rewrite it in place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedLink {
    pub text: String,
    pub raw_target: String,
    pub resolved: Option<PagePath>,
    pub target_start: usize,
    pub target_end: usize,
}

/// Scans `body` for inline Markdown links, resolving relative targets
/// against `page_path`'s parent directory.
#[must_use]
pub fn parse_links(page_path: &PagePath, body: &str) -> Vec<ParsedLink> {
    let base_dir = page_path
        .parent()
        .map_or_else(Vec::new, |parent| parent.segments().to_vec());

    let mut links = Vec::new();
    let bytes = body.as_bytes();
    let mut index = 0;
    while let Some(open_bracket) = body[index..].find('[') {
        let bracket_pos = index + open_bracket;
        if bracket_pos > 0 && bytes[bracket_pos - 1] == b'!' {
            // Skip image syntax; images are not treated as page links.
            index = bracket_pos + 1;
            continue;
        }
        let Some(close_bracket_rel) = body[bracket_pos + 1..].find(']') else {
            break;
        };
        let close_bracket = bracket_pos + 1 + close_bracket_rel;
        let after_close = close_bracket + 1;
        if body.as_bytes()[after_close..].first() != Some(&b'(') {
            index = bracket_pos + 1;
            continue;
        }
        let paren_open = after_close;
        let Some(close_paren_rel) = body[paren_open + 1..].find(')') else {
            break;
        };
        let close_paren = paren_open + 1 + close_paren_rel;

        let text = body[bracket_pos + 1..close_bracket].to_owned();
        let raw_target = body[paren_open + 1..close_paren].to_owned();
        let resolved = resolve_relative_target(&base_dir, &raw_target);

        links.push(ParsedLink {
            text,
            raw_target,
            resolved,
            target_start: paren_open + 1,
            target_end: close_paren,
        });

        index = close_paren + 1;
    }

    links
}

/// Resolves a raw inline link target against the directory containing the
/// referencing page.
///
/// Returns `None` for external URLs, anchors, `mailto:` links, and
/// root-relative (leading `/`) targets, which the first version does not
/// attempt to track.
#[must_use]
pub fn resolve_relative_target(base_dir: &[String], raw_target: &str) -> Option<PagePath> {
    let trimmed = raw_target.trim();
    let without_fragment = trimmed.split(['#', '?']).next().unwrap_or("");
    if without_fragment.is_empty() {
        return None;
    }
    if without_fragment.contains("://") || without_fragment.starts_with("mailto:") {
        return None;
    }
    if without_fragment.starts_with('/') {
        return None;
    }

    let mut segments: Vec<String> = base_dir.to_vec();
    for part in without_fragment.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other.to_owned()),
        }
    }
    if segments.is_empty() {
        return None;
    }

    PagePath::parse(&segments.join("/")).ok()
}

/// Computes the relative Markdown link target from the directory
/// `from_dir` to `to_path`, using `.md` and `../` conventions consistent
/// with the store's own inline links.
#[must_use]
pub fn relative_target(from_dir: &[String], to_path: &PagePath) -> String {
    let to_segments = to_path.segments();
    let to_dirs = &to_segments[..to_segments.len() - 1];

    let mut common = 0;
    while common < from_dir.len() && common < to_dirs.len() && from_dir[common] == to_dirs[common] {
        common += 1;
    }

    let ups = from_dir.len() - common;
    let mut parts: Vec<String> = std::iter::repeat_n("..".to_owned(), ups).collect();
    parts.extend(to_dirs[common..].iter().cloned());
    if let Some(last) = to_segments.last() {
        parts.push(last.clone());
    }

    format!("{}.md", parts.join("/"))
}

/// Rewrites every inline link in `body` that resolves to `source` so it
/// instead points at `destination`.
///
/// The replacement target is expressed relative to `page_path`'s directory.
/// Returns the rewritten body and whether anything changed.
#[must_use]
pub fn rewrite_links(
    page_path: &PagePath,
    body: &str,
    source: &PagePath,
    destination: &PagePath,
) -> (String, bool) {
    let base_dir = page_path
        .parent()
        .map_or_else(Vec::new, |parent| parent.segments().to_vec());
    let links = parse_links(page_path, body);

    let mut changed = false;
    let mut result = String::with_capacity(body.len());
    let mut cursor = 0;
    for link in links {
        if link.resolved.as_ref() != Some(source) {
            continue;
        }
        result.push_str(&body[cursor..link.target_start]);
        result.push_str(&relative_target(&base_dir, destination));
        cursor = link.target_end;
        changed = true;
    }
    result.push_str(&body[cursor..]);

    (result, changed)
}

#[cfg(test)]
mod tests {
    use super::{parse_links, relative_target, rewrite_links};
    use crate::path::PagePath;

    #[test]
    fn parses_simple_relative_link() {
        let page =
            PagePath::parse("people/alice").unwrap_or_else(|error| panic!("valid path: {error}"));
        let body = "Related: [AgentSpace](../projects/agentspace.md)";
        let links = parse_links(&page, body);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].text, "AgentSpace");
        assert_eq!(
            links[0].resolved.as_ref().map(PagePath::as_str),
            Some("projects/agentspace".to_owned())
        );
    }

    #[test]
    fn skips_images_and_external_links() {
        let page = PagePath::parse("notes/a").unwrap_or_else(|error| panic!("valid path: {error}"));
        let body = "![alt](image.png) and [external](https://example.com)";
        let links = parse_links(&page, body);
        assert_eq!(links.len(), 1);
        assert!(links[0].resolved.is_none());
    }

    #[test]
    fn computes_relative_target_across_directories() {
        let destination = PagePath::parse("projects/agentspace")
            .unwrap_or_else(|error| panic!("valid path: {error}"));
        let target = relative_target(&["people".to_owned()], &destination);
        assert_eq!(target, "../projects/agentspace.md");
    }

    #[test]
    fn computes_relative_target_same_directory() {
        let destination =
            PagePath::parse("people/bob").unwrap_or_else(|error| panic!("valid path: {error}"));
        let target = relative_target(&["people".to_owned()], &destination);
        assert_eq!(target, "bob.md");
    }

    #[test]
    fn rewrites_links_pointing_at_moved_page() {
        let referrer =
            PagePath::parse("people/alice").unwrap_or_else(|error| panic!("valid path: {error}"));
        let source = PagePath::parse("projects/agentspace")
            .unwrap_or_else(|error| panic!("valid path: {error}"));
        let destination = PagePath::parse("projects/renamed")
            .unwrap_or_else(|error| panic!("valid path: {error}"));
        let body = "See [AgentSpace](../projects/agentspace.md) for details.";
        let (new_body, changed) = rewrite_links(&referrer, body, &source, &destination);
        assert!(changed);
        assert_eq!(
            new_body,
            "See [AgentSpace](../projects/renamed.md) for details."
        );
    }

    #[test]
    fn leaves_unrelated_links_untouched() {
        let referrer =
            PagePath::parse("people/alice").unwrap_or_else(|error| panic!("valid path: {error}"));
        let source = PagePath::parse("projects/agentspace")
            .unwrap_or_else(|error| panic!("valid path: {error}"));
        let destination = PagePath::parse("projects/renamed")
            .unwrap_or_else(|error| panic!("valid path: {error}"));
        let body = "See [Other](../projects/other.md) for details.";
        let (new_body, changed) = rewrite_links(&referrer, body, &source, &destination);
        assert!(!changed);
        assert_eq!(new_body, body);
    }
}
