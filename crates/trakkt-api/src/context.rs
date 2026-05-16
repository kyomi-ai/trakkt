// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared helpers for API operations.
//!
//! These functions are extracted from the MCP tool handlers so that both the
//! MCP and REST surfaces can share the same resolution logic.

use crate::{ApiError, ApiResult};

/// Parse a compound issue identifier like `"ENG-42"` into `(team_key, number)`.
///
/// Returns `None` if the identifier does not match the expected
/// `<TEAM_KEY>-<NUMBER>` format.
pub fn parse_issue_identifier(identifier: &str) -> Option<(String, i32)> {
    let parts: Vec<&str> = identifier.splitn(2, '-').collect();
    if parts.len() == 2 && let Ok(number) = parts[1].parse::<i32>() {
        return Some((parts[0].to_string(), number));
    }
    None
}

/// Resolve a team to its `team_id` using either a direct ID or a short key.
///
/// - If `team_id` is provided, it is returned directly.
/// - If `team_key` is provided, the team is looked up by key within the
///   workspace. Returns an error if no team matches.
/// - If neither is provided, returns `None`.
pub async fn resolve_team(
    db: &trakkt_core::DbPool,
    workspace_id: &str,
    team_key: Option<&str>,
    team_id: Option<&str>,
) -> ApiResult<Option<String>> {
    if let Some(id) = team_id {
        return Ok(Some(id.to_string()));
    }
    if let Some(key) = team_key {
        let team = trakkt_auth::team_service::get_team_by_key(db, workspace_id, key)
            .await?
            .ok_or_else(|| ApiError::BadRequest(format!("No team found with key '{key}'")))?;
        return Ok(Some(team.team_id));
    }
    Ok(None)
}

/// Resolve team_key and issue number from either a compound identifier
/// (e.g. `"TRA-35"`) or explicit `team_key` + `issue_number` fields.
///
/// This is the typed-parameter equivalent of `resolve_issue_key_and_number`
/// in `routes/mcp.rs`, accepting `Option` fields from API param structs
/// rather than raw `serde_json::Value`.
pub fn resolve_issue_key_and_number(
    issue_identifier: Option<&str>,
    team_key: Option<&str>,
    issue_number: Option<i64>,
) -> ApiResult<(String, i32)> {
    if let Some(identifier) = issue_identifier {
        return parse_issue_identifier(identifier).ok_or_else(|| {
            ApiError::BadRequest(
                "Invalid issue identifier format. Expected 'TRA-35'".to_string(),
            )
        });
    }
    let key = team_key.ok_or_else(|| {
        ApiError::BadRequest(
            "Either issue_identifier or team_key+issue_number required".to_string(),
        )
    })?;
    let number = issue_number.ok_or_else(|| {
        ApiError::BadRequest(
            "Either issue_identifier or team_key+issue_number required".to_string(),
        )
    })? as i32;
    Ok((key.to_string(), number))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_identifier() {
        let result = parse_issue_identifier("ENG-42");
        assert_eq!(result, Some(("ENG".to_string(), 42)));
    }

    #[test]
    fn parse_multi_part_key() {
        // Only the first hyphen splits — "FOO-BAR-7" would fail since "BAR-7"
        // is not a valid integer. This is intentional: team keys are single
        // alphabetic tokens.
        let result = parse_issue_identifier("FOO-BAR-7");
        assert!(result.is_none());
    }

    #[test]
    fn parse_missing_number() {
        assert!(parse_issue_identifier("ENG-").is_none());
        assert!(parse_issue_identifier("ENG-abc").is_none());
    }

    #[test]
    fn parse_no_hyphen() {
        assert!(parse_issue_identifier("ENG42").is_none());
    }

    #[test]
    fn parse_negative_number() {
        // splitn(2, '-') on "ENG--5" gives ["ENG", "-5"], and "-5" parses as
        // i32 = -5. This is technically valid parsing, but callers should
        // validate positivity if needed.
        let result = parse_issue_identifier("ENG--5");
        assert_eq!(result, Some(("ENG".to_string(), -5)));
    }

    #[test]
    fn resolve_via_identifier() {
        let (key, num) =
            resolve_issue_key_and_number(Some("TRA-35"), None, None).unwrap();
        assert_eq!(key, "TRA");
        assert_eq!(num, 35);
    }

    #[test]
    fn resolve_via_key_and_number() {
        let (key, num) =
            resolve_issue_key_and_number(None, Some("ENG"), Some(7)).unwrap();
        assert_eq!(key, "ENG");
        assert_eq!(num, 7);
    }

    #[test]
    fn resolve_identifier_takes_precedence() {
        let (key, num) =
            resolve_issue_key_and_number(Some("TRA-10"), Some("ENG"), Some(99)).unwrap();
        assert_eq!(key, "TRA");
        assert_eq!(num, 10);
    }

    #[test]
    fn resolve_neither_returns_error() {
        let result = resolve_issue_key_and_number(None, None, None);
        assert!(result.is_err());
    }

    #[test]
    fn resolve_key_without_number_returns_error() {
        let result = resolve_issue_key_and_number(None, Some("TRA"), None);
        assert!(result.is_err());
    }
}
