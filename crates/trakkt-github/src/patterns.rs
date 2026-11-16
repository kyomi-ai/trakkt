// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pattern matching for issue references (e.g. `TRA-42`) in text.
//!
//! Extracts team-key + issue-number pairs from PR bodies, commit messages,
//! branch names, and other free-form text.

use std::collections::HashSet;
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
static ISSUE_REF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b([A-Z]{2,10})-(\d+)\b").expect("invalid issue ref regex"));

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

// ─── Link intent ────────────────────────────────────────────────────────────
//
// A key appearing in text is not a link. Linear's model (which we follow, see
// docs/ARCHITECTURE.md) links on *intent*: a magic word before the key, the
// branch name, or the PR title. Everything else is a mention.

/// What a link between a GitHub object and an issue is allowed to do.
///
/// Ordered weakest to strongest; when several sources name the same issue the
/// strongest wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LinkKind {
    /// Shown on the issue and the PR. Never changes status.
    Related,
    /// The key is in the branch name or the PR title. Starts the issue.
    BranchTitle,
    /// A non-closing magic word (`Refs`, `Part of`, ...). Starts the issue.
    Contributing,
    /// A closing magic word (`Fixes`, `Closes`, ...). Starts and completes.
    Closing,
}

impl LinkKind {
    /// The value stored in `github_links.link_kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Related => "related",
            Self::BranchTitle => "branch_title",
            Self::Contributing => "contributing",
            Self::Closing => "closing",
        }
    }

    /// Parse the stored value. Unknown values are an error, not a default.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "related" => Some(Self::Related),
            "branch_title" => Some(Self::BranchTitle),
            "contributing" => Some(Self::Contributing),
            "closing" => Some(Self::Closing),
            _ => None,
        }
    }
}

/// Words that link with closing behaviour. Exactly Linear's list.
const CLOSING_WORDS: &[&str] = &[
    "close",
    "closes",
    "closed",
    "closing",
    "fix",
    "fixes",
    "fixed",
    "fixing",
    "resolve",
    "resolves",
    "resolved",
    "resolving",
    "complete",
    "completes",
    "completed",
    "completing",
    "implement",
    "implements",
    "implemented",
    "implementing",
];
/// Words that link without completing. Exactly Linear's list.
const CONTRIBUTING_WORDS: &[&str] = &[
    "ref",
    "refs",
    "references",
    "part of",
    "contributes to",
    "toward",
    "towards",
];
/// Words that link for display only. Exactly Linear's list.
const RELATED_WORDS: &[&str] = &["relates to", "related to"];
/// Words that prevent linking altogether.
const SKIP_WORDS: &[&str] = &["skip", "ignore"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WordIntent {
    Link(LinkKind),
    Skip,
}

static MAGIC_WORD_RE: LazyLock<Regex> = LazyLock::new(|| {
    let mut words: Vec<(String, WordIntent)> = Vec::new();
    for w in CLOSING_WORDS {
        words.push(((*w).into(), WordIntent::Link(LinkKind::Closing)));
    }
    for w in CONTRIBUTING_WORDS {
        words.push(((*w).into(), WordIntent::Link(LinkKind::Contributing)));
    }
    for w in RELATED_WORDS {
        words.push(((*w).into(), WordIntent::Link(LinkKind::Related)));
    }
    for w in SKIP_WORDS {
        words.push(((*w).into(), WordIntent::Skip));
    }
    // Longest first so `fixes` is not read as `fix` + `es`.
    words.sort_by_key(|(w, _)| std::cmp::Reverse(w.len()));
    let alternation = words
        .iter()
        .map(|(w, _)| regex::escape(w).replace(' ', r"\s+"))
        .collect::<Vec<_>>()
        .join("|");
    Regex::new(&format!(r"(?i)\b({alternation})\b")).expect("invalid magic word regex")
});

fn word_intent(word: &str) -> Option<WordIntent> {
    let normalized = word
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let is = |list: &[&str]| list.contains(&normalized.as_str());
    if is(CLOSING_WORDS) {
        Some(WordIntent::Link(LinkKind::Closing))
    } else if is(CONTRIBUTING_WORDS) {
        Some(WordIntent::Link(LinkKind::Contributing))
    } else if is(RELATED_WORDS) {
        Some(WordIntent::Link(LinkKind::Related))
    } else if is(SKIP_WORDS) {
        Some(WordIntent::Skip)
    } else {
        None
    }
}

static BARE_KEY_TOKEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?i)([A-Z]{2,10})-(\d+)$").expect("invalid key token regex"));
static ISSUE_URL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?i)https?://([^/\s]+)/(?:\S*?/)?issues/([A-Z]{2,10})-(\d+)(?:[/?#]\S*)?$")
        .expect("invalid issue url regex")
});

/// The host of an application URL, for matching issue URLs against.
fn host_of(url: &str) -> Option<String> {
    let rest = url.split("://").nth(1)?;
    Some(rest.split(['/', '?', '#']).next()?.to_lowercase())
}

/// Read one issue reference from a token: a bare key, or a full issue URL on
/// this application's host.
fn ref_from_token(token: &str, app_host: Option<&str>) -> Option<IssueRef> {
    let token = token
        .trim_start_matches(['(', '[', '<'])
        .trim_end_matches(['.', ')', ']', '>', ';', ':', '!']);
    let (key, number) = if let Some(c) = BARE_KEY_TOKEN_RE.captures(token) {
        (c[1].to_uppercase(), c[2].parse().ok()?)
    } else {
        let c = ISSUE_URL_RE.captures(token)?;
        // A URL on another product's host is not one of our issues.
        if app_host? != c[1].to_lowercase() {
            return None;
        }
        (c[2].to_uppercase(), c[3].parse().ok()?)
    };
    Some(IssueRef {
        team_key: key,
        number,
    })
}

/// Read the list of references following a magic word:
/// `Fixes ENG-123, DES-5, and ENG-256`. Stops at the first thing that is not
/// a reference, so `Fixes the bug in TRA-5` links nothing.
fn read_ref_list(rest: &str, app_host: Option<&str>) -> Vec<IssueRef> {
    let mut found = Vec::new();
    let mut rest = rest.trim_start_matches([' ', '\t']);
    rest = rest.strip_prefix(':').unwrap_or(rest);
    loop {
        rest = rest.trim_start_matches([' ', '\t']);
        let end = rest
            .find(|c: char| c.is_whitespace() || c == ',')
            .unwrap_or(rest.len());
        let Some(issue_ref) = ref_from_token(&rest[..end], app_host) else {
            break;
        };
        found.push(issue_ref);
        rest = &rest[end..];
        // Separators between references: commas, `&`, `and`.
        loop {
            let trimmed = rest.trim_start_matches([' ', '\t', ',', '&']);
            let lowered = trimmed.get(..4).map(str::to_lowercase);
            if lowered.as_deref() == Some("and ") {
                rest = &trimmed[4..];
            } else if trimmed.len() != rest.len() {
                rest = trimmed;
            } else {
                break;
            }
        }
    }
    found
}

/// The outcome of reading one piece of text.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TextIntent {
    /// References with the strongest kind the text gives each, in order of
    /// first appearance.
    pub refs: Vec<(IssueRef, LinkKind)>,
    /// References preceded by `skip` or `ignore`.
    pub skipped: HashSet<IssueRef>,
}

/// Read the links in `text`. A reference behind a magic word gets that word's
/// kind; a reference with no magic word gets `bare`.
pub fn parse_text(text: &str, bare: LinkKind, app_url: &str) -> TextIntent {
    let app_host = host_of(app_url);
    let mut kinds: Vec<(IssueRef, LinkKind)> = Vec::new();
    let mut skipped = HashSet::new();
    let mut raise =
        |issue_ref: IssueRef, kind: LinkKind| match kinds.iter_mut().find(|(r, _)| *r == issue_ref)
        {
            Some((_, existing)) => *existing = (*existing).max(kind),
            None => kinds.push((issue_ref, kind)),
        };
    for m in MAGIC_WORD_RE.find_iter(text) {
        let Some(intent) = word_intent(m.as_str()) else {
            continue;
        };
        for issue_ref in read_ref_list(&text[m.end()..], app_host.as_deref()) {
            match intent {
                WordIntent::Link(kind) => raise(issue_ref, kind),
                WordIntent::Skip => {
                    skipped.insert(issue_ref);
                }
            }
        }
    }
    for issue_ref in extract_issue_refs(text) {
        raise(issue_ref, bare);
    }
    kinds.retain(|(r, _)| !skipped.contains(r));
    TextIntent {
        refs: kinds,
        skipped,
    }
}

/// Combine the pieces of one GitHub object (title, body, branch, ...) into
/// the final link set: the strongest kind per issue, and `skip` in any piece
/// removing the issue altogether, branch name included.
pub fn combine(parts: Vec<TextIntent>) -> Vec<(IssueRef, LinkKind)> {
    let skipped: HashSet<IssueRef> = parts.iter().flat_map(|p| p.skipped.clone()).collect();
    let mut combined: Vec<(IssueRef, LinkKind)> = Vec::new();
    for (issue_ref, kind) in parts.into_iter().flat_map(|p| p.refs) {
        if skipped.contains(&issue_ref) {
            continue;
        }
        match combined.iter_mut().find(|(r, _)| *r == issue_ref) {
            Some((_, existing)) => *existing = (*existing).max(kind),
            None => combined.push((issue_ref, kind)),
        }
    }
    combined
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
        assert_eq!(
            refs[0],
            IssueRef {
                team_key: "TRA".to_string(),
                number: 42
            }
        );
        assert_eq!(
            refs[1],
            IssueRef {
                team_key: "TRA".to_string(),
                number: 43
            }
        );
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
        assert_eq!(
            refs[0],
            IssueRef {
                team_key: "TRA".to_string(),
                number: 42
            }
        );
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
    fn empty_text() {
        assert!(extract_issue_refs("").is_empty());
    }

    #[test]
    fn no_refs_in_regular_text() {
        assert!(extract_issue_refs("Just a regular commit message without references").is_empty());
    }

    // ── link intent ───────────────────────────────────────────────────────

    const APP: &str = "https://trakkt.example";

    fn r(key: &str, number: i32) -> IssueRef {
        IssueRef {
            team_key: key.to_string(),
            number,
        }
    }

    fn kinds(text: &str, bare: LinkKind) -> Vec<(IssueRef, LinkKind)> {
        parse_text(text, bare, APP).refs
    }

    #[test]
    fn every_closing_word_closes() {
        for word in CLOSING_WORDS {
            for text in [
                format!("{word} TRA-1"),
                format!("{} TRA-1", word.to_uppercase()),
            ] {
                assert_eq!(
                    kinds(&text, LinkKind::Related),
                    vec![(r("TRA", 1), LinkKind::Closing)],
                    "{text}"
                );
            }
        }
    }

    #[test]
    fn every_contributing_word_contributes() {
        for word in CONTRIBUTING_WORDS {
            assert_eq!(
                kinds(&format!("{word} TRA-1"), LinkKind::Related),
                vec![(r("TRA", 1), LinkKind::Contributing)],
                "{word}"
            );
        }
    }

    #[test]
    fn every_relation_word_relates_only() {
        for word in RELATED_WORDS {
            assert_eq!(
                kinds(&format!("{word} TRA-1"), LinkKind::Related),
                vec![(r("TRA", 1), LinkKind::Related)],
                "{word}"
            );
        }
    }

    #[test]
    fn skip_and_ignore_remove_the_reference_entirely() {
        for word in SKIP_WORDS {
            let intent = parse_text(
                &format!("{word} TRA-1 and Fixes TRA-2"),
                LinkKind::Related,
                APP,
            );
            assert_eq!(intent.refs, vec![(r("TRA", 2), LinkKind::Closing)]);
            assert!(intent.skipped.contains(&r("TRA", 1)));
        }
    }

    #[test]
    fn several_references_after_one_word() {
        assert_eq!(
            kinds("Fixes ENG-123, DES-5, and ENG-256", LinkKind::Related),
            vec![
                (r("ENG", 123), LinkKind::Closing),
                (r("DES", 5), LinkKind::Closing),
                (r("ENG", 256), LinkKind::Closing),
            ]
        );
        assert_eq!(
            kinds("refs ENG-1 & ENG-2 and ENG-3", LinkKind::Related),
            vec![
                (r("ENG", 1), LinkKind::Contributing),
                (r("ENG", 2), LinkKind::Contributing),
                (r("ENG", 3), LinkKind::Contributing),
            ]
        );
    }

    #[test]
    fn the_list_stops_at_the_first_non_reference() {
        assert_eq!(
            kinds("Fixes TRA-1 and the crash, see TRA-2", LinkKind::Related),
            vec![
                (r("TRA", 1), LinkKind::Closing),
                (r("TRA", 2), LinkKind::Related)
            ]
        );
    }

    #[test]
    fn a_word_not_directly_before_the_key_is_only_a_mention() {
        assert_eq!(
            kinds("Fixes the crash described in TRA-5", LinkKind::Related),
            vec![(r("TRA", 5), LinkKind::Related)]
        );
        assert_eq!(
            kinds("prefix TRA-5", LinkKind::Related),
            vec![(r("TRA", 5), LinkKind::Related)],
            "`fix` inside `prefix` is not a magic word"
        );
    }

    #[test]
    fn colon_and_trailing_punctuation_are_tolerated() {
        assert_eq!(
            kinds("Fixes: TRA-1.", LinkKind::Related),
            vec![(r("TRA", 1), LinkKind::Closing)]
        );
        assert_eq!(
            kinds("(Closes TRA-1)", LinkKind::Related),
            vec![(r("TRA", 1), LinkKind::Closing)]
        );
    }

    #[test]
    fn a_full_issue_url_on_this_host_links_and_one_elsewhere_does_not() {
        assert_eq!(
            kinds(
                "Fixes https://trakkt.example/issues/TRA-9",
                LinkKind::Related
            )[0],
            (r("TRA", 9), LinkKind::Closing)
        );
        let elsewhere = kinds(
            "Fixes https://linear.app/acme/issue/TRA-9/slug",
            LinkKind::Related,
        );
        assert!(
            elsewhere.iter().all(|(_, k)| *k == LinkKind::Related),
            "another product's URL must not carry closing intent: {elsewhere:?}"
        );
        let other_host = kinds(
            "Fixes https://other.example/issues/TRA-9",
            LinkKind::Related,
        );
        assert!(other_host.iter().all(|(_, k)| *k == LinkKind::Related));
    }

    #[test]
    fn a_bare_key_gets_the_callers_kind_and_a_word_overrides_it() {
        assert_eq!(
            kinds("see TRA-1", LinkKind::Related),
            vec![(r("TRA", 1), LinkKind::Related)]
        );
        assert_eq!(
            kinds("TRA-1: tidy", LinkKind::BranchTitle),
            vec![(r("TRA", 1), LinkKind::BranchTitle)]
        );
        assert_eq!(
            kinds("Related to TRA-1", LinkKind::BranchTitle),
            vec![(r("TRA", 1), LinkKind::BranchTitle)],
            "the strongest source wins, a relation word never weakens a title link"
        );
    }

    #[test]
    fn combine_takes_the_strongest_kind_and_skip_beats_the_branch() {
        let title = parse_text("TRA-1 and TRA-2", LinkKind::BranchTitle, APP);
        let body = parse_text(
            "Fixes TRA-1. Ignore TRA-2. Mentions TRA-3",
            LinkKind::Related,
            APP,
        );
        let branch = parse_text("tra-2-thing", LinkKind::BranchTitle, APP);
        assert_eq!(
            combine(vec![title, body, branch]),
            vec![
                (r("TRA", 1), LinkKind::Closing),
                (r("TRA", 3), LinkKind::Related)
            ]
        );
    }

    #[test]
    fn link_kind_round_trips_and_rejects_unknown_values() {
        for kind in [
            LinkKind::Related,
            LinkKind::BranchTitle,
            LinkKind::Contributing,
            LinkKind::Closing,
        ] {
            assert_eq!(LinkKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(LinkKind::parse("closes"), None);
    }
}
