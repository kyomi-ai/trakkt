// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pattern matching for issue references (e.g. `TRA-42`) in text.
//!
//! Extracts team-key + issue-number pairs from PR bodies, commit messages,
//! branch names, and other free-form text.

use std::sync::LazyLock;

use regex::Regex;

/// A parsed issue reference consisting of a team key and issue number.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IssueRef {
    /// Uppercase team key (e.g. "TRA", "KYO").
    pub team_key: String,
    /// Issue number within the team.
    pub number: i32,
}

/// Regex matching `TEAM-N` patterns.
///
/// Uses word boundary (`\b`) to avoid false positives like "EXTRA-1" matching
/// as "A-1". The team key must be 2-10 uppercase/lowercase ASCII letters.
static ISSUE_REF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b([A-Z]{2,10})-(\d+)\b").expect("invalid issue ref regex")
});

/// Regex matching close-intent keywords immediately before an issue reference.
///
/// Matches patterns like "Closes TRA-42", "fixes TRA-7", "Resolves KYO-123".
/// Allows optional colon and whitespace between keyword and reference.
static CLOSE_INTENT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:closes|fixes|resolves)\s*:?\s*([A-Z]{2,10})-(\d+)\b")
        .expect("invalid close intent regex")
});

/// Extract all issue references from the given text.
///
/// Returns a de-duplicated list of `IssueRef` values. Team keys are normalized
/// to uppercase regardless of the case used in the source text.
pub fn extract_issue_refs(text: &str) -> Vec<IssueRef> {
    let mut seen = std::collections::HashSet::new();
    let mut results = Vec::new();

    for cap in ISSUE_REF_RE.captures_iter(text) {
        let team_key = cap[1].to_uppercase();
        let number: i32 = match cap[2].parse() {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(raw = &cap[2], error = %e, "skipping issue ref with unparseable number");
                continue;
            }
        };

        let issue_ref = IssueRef { team_key, number };
        if seen.insert(issue_ref.clone()) {
            results.push(issue_ref);
        }
    }

    results
}

/// Extract issue references that are preceded by a close-intent keyword.
///
/// Only returns references preceded by "closes", "fixes", or "resolves"
/// (case-insensitive). Results are de-duplicated.
pub fn extract_close_intent_refs(text: &str) -> Vec<IssueRef> {
    let mut seen = std::collections::HashSet::new();
    let mut results = Vec::new();

    for cap in CLOSE_INTENT_RE.captures_iter(text) {
        let team_key = cap[1].to_uppercase();
        let number: i32 = match cap[2].parse() {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(raw = &cap[2], error = %e, "skipping issue ref with unparseable number");
                continue;
            }
        };

        let issue_ref = IssueRef { team_key, number };
        if seen.insert(issue_ref.clone()) {
            results.push(issue_ref);
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_extraction() {
        let refs = extract_issue_refs("Fixes TRA-42 by updating the schema");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].team_key, "TRA");
        assert_eq!(refs[0].number, 42);
    }

    #[test]
    fn branch_name_extraction() {
        let refs = extract_issue_refs("tra-7-fix-login-timeout");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].team_key, "TRA");
        assert_eq!(refs[0].number, 7);
    }

    #[test]
    fn multiple_refs() {
        let refs = extract_issue_refs("Implements TRA-42 and TRA-43");
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0], IssueRef { team_key: "TRA".to_string(), number: 42 });
        assert_eq!(refs[1], IssueRef { team_key: "TRA".to_string(), number: 43 });
    }

    #[test]
    fn different_team_keys() {
        let refs = extract_issue_refs("Closes KYO-123 and TRA-5");
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].team_key, "KYO");
        assert_eq!(refs[0].number, 123);
        assert_eq!(refs[1].team_key, "TRA");
        assert_eq!(refs[1].number, 5);
    }

    #[test]
    fn case_insensitive_normalized_to_uppercase() {
        let refs = extract_issue_refs("tra-42 kyo-10 TRA-99");
        assert_eq!(refs.len(), 3);
        assert_eq!(refs[0].team_key, "TRA");
        assert_eq!(refs[1].team_key, "KYO");
        assert_eq!(refs[2].team_key, "TRA");
    }

    #[test]
    fn deduplication() {
        let refs = extract_issue_refs("TRA-42 mentioned twice: TRA-42");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0], IssueRef { team_key: "TRA".to_string(), number: 42 });
    }

    #[test]
    fn deduplication_case_insensitive() {
        let refs = extract_issue_refs("tra-42 and TRA-42 are the same");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].team_key, "TRA");
        assert_eq!(refs[0].number, 42);
    }

    #[test]
    fn no_false_positives_from_longer_words() {
        // "EXTRA-1" should NOT match as "A-1" — the word boundary prevents it
        let refs = extract_issue_refs("EXTRA-1 should match as EXTRA team key");
        // It DOES match as team_key=EXTRA, number=1 because EXTRA is 5 letters
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].team_key, "EXTRA");
        assert_eq!(refs[0].number, 1);
    }

    #[test]
    fn no_match_for_single_letter_prefix() {
        // Single letter prefix should not match (minimum 2 letters for team key)
        let refs = extract_issue_refs("X-1 is not a valid team key");
        assert_eq!(refs.len(), 0);
    }

    #[test]
    fn no_match_for_numbers_prefix() {
        let refs = extract_issue_refs("123-456 is not a team ref");
        assert_eq!(refs.len(), 0);
    }

    #[test]
    fn close_intent_fixes() {
        let refs = extract_close_intent_refs("Fixes TRA-42 by updating schema");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0], IssueRef { team_key: "TRA".to_string(), number: 42 });
    }

    #[test]
    fn close_intent_closes() {
        let refs = extract_close_intent_refs("Closes KYO-123");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].team_key, "KYO");
        assert_eq!(refs[0].number, 123);
    }

    #[test]
    fn close_intent_resolves() {
        let refs = extract_close_intent_refs("resolves TRA-7");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].team_key, "TRA");
        assert_eq!(refs[0].number, 7);
    }

    #[test]
    fn close_intent_with_colon() {
        let refs = extract_close_intent_refs("Fixes: TRA-42");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].team_key, "TRA");
        assert_eq!(refs[0].number, 42);
    }

    #[test]
    fn close_intent_does_not_match_plain_refs() {
        // "Implements TRA-42" should NOT be in close intent results
        let refs = extract_close_intent_refs("Implements TRA-42 and closes TRA-43");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0], IssueRef { team_key: "TRA".to_string(), number: 43 });
    }

    #[test]
    fn close_intent_case_insensitive() {
        let refs = extract_close_intent_refs("FIXES tra-10");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].team_key, "TRA");
        assert_eq!(refs[0].number, 10);
    }

    #[test]
    fn empty_text() {
        assert!(extract_issue_refs("").is_empty());
        assert!(extract_close_intent_refs("").is_empty());
    }

    #[test]
    fn no_refs_in_regular_text() {
        assert!(extract_issue_refs("Just a regular commit message without references").is_empty());
    }
}
