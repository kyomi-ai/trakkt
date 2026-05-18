// SPDX-License-Identifier: AGPL-3.0-or-later

//! Auto-link issue identifiers (e.g. `TRA-109`, `KOD-42`) into clickable links.
//!
//! Two public functions serve different surfaces:
//! - [`auto_link_issue_identifiers`] — transforms markdown text, inserting markdown links
//! - [`auto_link_view`] — transforms plain text into a Leptos view with `<a>` elements

use std::ops::Range;
use std::sync::LazyLock;

use leptos::prelude::*;
use regex::Regex;

/// Pattern matching issue identifiers: 2-10 uppercase letters, dash, one or more digits.
static IDENTIFIER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Z]{2,10}-\d+").expect("identifier regex"));

/// Pattern matching fenced code blocks (``` ... ```).
static CODE_BLOCK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)```.*?```").expect("code block regex"));

/// Pattern matching inline code (` ... `), non-greedy, single-line.
static INLINE_CODE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`[^`]+`").expect("inline code regex"));

/// Pattern matching markdown links `[text](url)`.
static MD_LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[(?:[^\]]*)\]\([^)]*\)").expect("markdown link regex"));

/// Takes markdown text, finds issue identifier patterns, and replaces them
/// with markdown links `[MATCH](/issues/MATCH)`.
///
/// Skips identifiers inside:
/// - Fenced code blocks (triple backticks)
/// - Inline code (single backticks)
/// - Existing markdown links `[text](url)`
///
/// Returns the original text unchanged if no replacements were made.
pub fn auto_link_issue_identifiers(text: &str) -> String {
    // 1. Collect all protected byte ranges (code blocks, inline code, markdown links).
    let mut protected: Vec<Range<usize>> = Vec::new();

    for m in CODE_BLOCK_RE.find_iter(text) {
        protected.push(m.start()..m.end());
    }
    for m in INLINE_CODE_RE.find_iter(text) {
        protected.push(m.start()..m.end());
    }
    for m in MD_LINK_RE.find_iter(text) {
        protected.push(m.start()..m.end());
    }

    // 2. Find all identifier matches.
    let matches: Vec<regex::Match<'_>> = IDENTIFIER_RE.find_iter(text).collect();

    if matches.is_empty() {
        return text.to_string();
    }

    // 3. Filter to only identifiers NOT inside any protected range.
    let replaceable: Vec<&regex::Match<'_>> = matches
        .iter()
        .filter(|m| {
            !protected
                .iter()
                .any(|p| m.start() >= p.start && m.end() <= p.end)
        })
        .collect();

    if replaceable.is_empty() {
        return text.to_string();
    }

    // 4. Build output string with replacements (process from end to preserve byte offsets).
    let mut result = text.to_string();
    for m in replaceable.into_iter().rev() {
        let identifier = m.as_str();
        let link = format!("[{identifier}](/issues/{identifier})");
        result.replace_range(m.start()..m.end(), &link);
    }

    result
}

/// Takes plain text (NOT markdown), finds issue identifier patterns, and returns
/// a Leptos view with identifiers wrapped in `<a>` elements.
///
/// Used for activity entries where content is plain text rendered directly in views.
/// Links use `text-accent-foreground hover:underline` (teal from the design system).
pub fn auto_link_view(text: &str) -> AnyView {
    let matches: Vec<regex::Match<'_>> = IDENTIFIER_RE.find_iter(text).collect();

    if matches.is_empty() {
        let owned = text.to_string();
        return view! { <span>{owned}</span> }.into_any();
    }

    let mut fragments: Vec<AnyView> = Vec::new();
    let mut last_end = 0;

    for m in &matches {
        // Add text before this match.
        if m.start() > last_end {
            let before = text[last_end..m.start()].to_string();
            fragments.push(view! { <span>{before}</span> }.into_any());
        }

        // Add the linked identifier.
        let identifier = m.as_str().to_string();
        let href = format!("/issues/{identifier}");
        fragments.push(
            view! {
                <a
                    href=href
                    class="text-accent-foreground hover:underline font-medium transition-colors duration-200 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring rounded-sm"
                >{identifier}</a>
            }
            .into_any(),
        );

        last_end = m.end();
    }

    // Add any trailing text after the last match.
    if last_end < text.len() {
        let after = text[last_end..].to_string();
        fragments.push(view! { <span>{after}</span> }.into_any());
    }

    view! { <span>{fragments}</span> }.into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_replacement() {
        let input = "See TRA-109 for details";
        let result = auto_link_issue_identifiers(input);
        assert_eq!(result, "See [TRA-109](/issues/TRA-109) for details");
    }

    #[test]
    fn test_multiple_identifiers() {
        let input = "TRA-1 and KOD-42 are related";
        let result = auto_link_issue_identifiers(input);
        assert_eq!(
            result,
            "[TRA-1](/issues/TRA-1) and [KOD-42](/issues/KOD-42) are related"
        );
    }

    #[test]
    fn test_no_identifiers() {
        let input = "No issue references here";
        let result = auto_link_issue_identifiers(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_skip_inline_code() {
        let input = "See `TRA-109` in the code";
        let result = auto_link_issue_identifiers(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_skip_code_block() {
        let input = "```\nTRA-109\n```";
        let result = auto_link_issue_identifiers(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_skip_existing_link() {
        let input = "See [TRA-109](/issues/TRA-109) already linked";
        let result = auto_link_issue_identifiers(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_skip_link_in_url() {
        let input = "Check [details](/issues/TRA-109) for more";
        let result = auto_link_issue_identifiers(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_mixed_protected_and_unprotected() {
        let input = "Fix TRA-1, see `TRA-2` in code, and TRA-3";
        let result = auto_link_issue_identifiers(input);
        assert_eq!(
            result,
            "Fix [TRA-1](/issues/TRA-1), see `TRA-2` in code, and [TRA-3](/issues/TRA-3)"
        );
    }

    #[test]
    fn test_identifier_at_boundaries() {
        let input = "TRA-109";
        let result = auto_link_issue_identifiers(input);
        assert_eq!(result, "[TRA-109](/issues/TRA-109)");
    }

    #[test]
    fn test_idempotent() {
        // Running auto_link on already-linked text should produce the same output
        // (the generated markdown links are protected regions).
        let input = "See TRA-109 for details";
        let first_pass = auto_link_issue_identifiers(input);
        let second_pass = auto_link_issue_identifiers(&first_pass);
        assert_eq!(first_pass, second_pass);
    }

    #[test]
    fn test_empty_string() {
        let result = auto_link_issue_identifiers("");
        assert_eq!(result, "");
    }

    #[test]
    fn test_identifier_in_link_text() {
        // Identifier inside the text portion of an existing markdown link
        let input = "See [TRA-109 details](https://example.com) for more";
        let result = auto_link_issue_identifiers(input);
        assert_eq!(result, input);
    }
}
