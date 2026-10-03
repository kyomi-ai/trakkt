// SPDX-License-Identifier: AGPL-3.0-or-later

//! Search service — full-text search across issues and comments.
//!
//! On Postgres, uses `tsvector` columns with GIN indexes for ranked full-text
//! search with snippet highlighting. On SQLite, falls back to `LIKE` matching
//! without ranking or snippets.

use trakkt_core::DbPool;

// ─── Row types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, sqlx::FromRow)]
struct SearchResultRow {
    pub issue_id: String,
    pub number: i64,
    pub team_key: String,
    pub title: String,
    pub status_name: String,
    pub status_category: String,
    pub priority: i32,
    pub snippet: Option<String>,
    pub match_field: String,
    pub rank: f64,
}

/// DTO for search results returned to API consumers.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchResult {
    pub issue_id: String,
    pub number: i64,
    pub team_key: String,
    pub title: String,
    pub status_name: String,
    pub status_category: String,
    pub priority: i32,
    pub snippet: Option<String>,
    pub match_field: String,
    pub rank: f64,
}

/// Search response with total count for pagination.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
    pub total: i64,
}

impl From<SearchResultRow> for SearchResult {
    fn from(row: SearchResultRow) -> Self {
        Self {
            issue_id: row.issue_id,
            number: row.number,
            team_key: row.team_key,
            title: row.title,
            status_name: row.status_name,
            status_category: row.status_category,
            priority: row.priority,
            snippet: row.snippet,
            match_field: row.match_field,
            rank: row.rank,
        }
    }
}

/// Parameters controlling a search query.
pub struct SearchParams {
    pub query: String,
    pub workspace_id: String,
    pub team_id: Option<String>,
    pub include_archived: bool,
    pub include_closed: bool,
    pub include_comments: bool,
    pub limit: i64,
    pub offset: i64,
}

// ─── Main search function ───────────────────────────────────────────────────

/// Search issues (and optionally comments) by text query.
///
/// On Postgres, uses `tsvector @@ plainto_tsquery` with `ts_rank` for ranked
/// results and `ts_headline` for snippet context. On SQLite, falls back to
/// `LIKE '%query%'` without ranking or snippets.
///
/// Returns a [`SearchResponse`] containing the paginated results and the total
/// number of matching items (before offset/limit are applied). Pagination is
/// performed in Rust so that `total` reflects the true match count. The SQL
/// queries are run with a high cap (1000 rows) and no offset; deduplication
/// and slicing happen after.
pub async fn search(
    db: &DbPool,
    params: &SearchParams,
) -> trakkt_core::Result<SearchResponse> {
    if params.query.trim().is_empty() {
        return Ok(SearchResponse {
            results: Vec::new(),
            total: 0,
        });
    }

    let identifier = parse_identifier_number(&params.query);

    // Fetch all matching rows (up to a reasonable cap) so we can report the
    // true total count. SQL LIMIT/OFFSET are bypassed here; pagination is
    // applied in Rust below.
    let uncapped_params = SearchParams {
        query: params.query.clone(),
        workspace_id: params.workspace_id.clone(),
        team_id: params.team_id.clone(),
        include_archived: params.include_archived,
        include_closed: params.include_closed,
        include_comments: params.include_comments,
        limit: 1000,
        offset: 0,
    };

    let rows = if db.is_postgres() {
        search_postgres(db, &uncapped_params, &identifier).await?
    } else {
        search_sqlite(db, &uncapped_params, &identifier).await?
    };

    // Dedup by issue_id, keeping the entry with the highest rank (identifier
    // matches have rank 2.0 so they win over tsvector / LIKE matches).
    let mut seen = std::collections::HashSet::new();
    let all_results: Vec<SearchResult> = rows
        .into_iter()
        .map(SearchResult::from)
        .filter(|r| seen.insert(r.issue_id.clone()))
        .collect();

    let total = all_results.len() as i64;

    // Apply pagination in Rust.
    let offset = params.offset as usize;
    let limit = params.limit as usize;
    let results = all_results.into_iter().skip(offset).take(limit).collect();

    Ok(SearchResponse { results, total })
}

// ─── Shared condition builders ─────────────────────────────────────────────

/// Build the WHERE conditions shared between issue and comment arms (Postgres).
/// Workspace is always `$2`, team_id is `$3` when present.
fn shared_conditions_pg(params: &SearchParams) -> Vec<String> {
    let mut conds = vec!["i.workspace_id = $2".to_string()];
    if !params.include_closed {
        conds.push("s.category NOT IN ('completed', 'cancelled')".to_string());
    }
    if params.team_id.is_some() {
        conds.push("i.team_id = $3".to_string());
    }
    if !params.include_archived {
        conds.push("i.archived_at IS NULL".to_string());
    }
    conds
}

/// Build the WHERE conditions shared between issue and comment arms (SQLite).
/// Workspace is always `$1`, team_id is `$3` when present.
fn shared_conditions_sqlite(params: &SearchParams) -> Vec<String> {
    let mut conds = vec!["i.workspace_id = $1".to_string()];
    if !params.include_closed {
        conds.push("s.category NOT IN ('completed', 'cancelled')".to_string());
    }
    if params.team_id.is_some() {
        conds.push("i.team_id = $3".to_string());
    }
    if !params.include_archived {
        conds.push("i.archived_at IS NULL".to_string());
    }
    conds
}

/// Parse an issue identifier from a search query.
///
/// Returns `Some((team_key, number))` when the query looks like an identifier:
/// - `"TRA-216"` → `Some((Some("TRA"), 216))`
/// - `"tra-216"` → `Some((Some("TRA"), 216))`
/// - `"216"`     → `Some((None, 216))`
/// - `"hello"`   → `None`
fn parse_identifier_number(query: &str) -> Option<(Option<String>, i64)> {
    let trimmed = query.trim();

    // Try `{LETTERS}-{DIGITS}` pattern first (case-insensitive).
    if let Some((prefix, suffix)) = trimmed.split_once('-')
        && !prefix.is_empty()
        && prefix.chars().all(|c| c.is_ascii_alphabetic())
        && !suffix.is_empty()
        && let Ok(n) = suffix.parse::<i64>()
        && n > 0
    {
        return Some((Some(prefix.to_ascii_uppercase()), n));
    }

    // Try bare positive number.
    if let Ok(n) = trimmed.parse::<i64>()
        && n > 0
    {
        return Some((None, n));
    }

    None
}

/// Escape LIKE special characters and wrap with wildcards for SQLite search.
fn escape_like_pattern(query: &str) -> String {
    let escaped = query
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

// ─── Postgres implementation ────────────────────────────────────────────────

async fn search_postgres(
    db: &DbPool,
    params: &SearchParams,
    identifier: &Option<(Option<String>, i64)>,
) -> trakkt_core::Result<Vec<SearchResultRow>> {
    // Bind positions:
    //   $1 = search query text
    //   $2 = workspace_id
    //   $3 = team_id (when team filter is active)
    //   $N = identifier number (when identifier detected)
    //   $N+1 = parsed team_key (when identifier has "KEY-NUM" format)

    let mut next_param = if params.team_id.is_some() { 4 } else { 3 };

    let mut issue_conditions = shared_conditions_pg(params);
    issue_conditions.push("i.search_vector @@ query".to_string());
    let issue_where = issue_conditions.join(" AND ");

    let issue_sql = format!(
        "SELECT i.issue_id, CAST(i.number AS BIGINT) AS number, t.key AS team_key, i.title, \
         s.name AS status_name, s.category AS status_category, \
         i.priority, \
         ts_headline('english', coalesce(i.title, '') || ' ' || coalesce(i.description, ''), query, \
         'MaxWords=35, MinWords=15, StartSel=**, StopSel=**') AS snippet, \
         CASE WHEN to_tsvector('english', coalesce(i.title, '')) @@ query THEN 'title' \
         ELSE 'description' END AS match_field, \
         ts_rank(i.search_vector, query)::FLOAT8 AS rank \
         FROM issues i \
         JOIN teams t ON t.team_id = i.team_id \
         JOIN statuses s ON s.status_id = i.status_id \
         CROSS JOIN plainto_tsquery('english', $1) query \
         WHERE {issue_where}"
    );

    // Build the identifier UNION arm when the query looks like an identifier.
    let identifier_sql = if let Some((parsed_key, _number)) = identifier {
        let number_param = format!("${next_param}");
        next_param += 1;

        let mut id_conditions = shared_conditions_pg(params);
        id_conditions.push(format!("i.number = {number_param}"));

        // When the query contained a team key (e.g. "TRA-216"), also match team.
        if parsed_key.is_some() {
            let key_param = format!("${next_param}");
            id_conditions.push(format!("UPPER(t.key) = {key_param}"));
        }

        let id_where = id_conditions.join(" AND ");
        Some(format!(
            "SELECT i.issue_id, CAST(i.number AS BIGINT) AS number, t.key AS team_key, i.title, \
             s.name AS status_name, s.category AS status_category, \
             i.priority, \
             NULL AS snippet, \
             'identifier' AS match_field, \
             2.0::FLOAT8 AS rank \
             FROM issues i \
             JOIN teams t ON t.team_id = i.team_id \
             JOIN statuses s ON s.status_id = i.status_id \
             WHERE {id_where}"
        ))
    } else {
        None
    };

    let comment_sql = if params.include_comments {
        let mut comment_conditions = shared_conditions_pg(params);
        comment_conditions.push("c.search_vector @@ query".to_string());
        let comment_where = comment_conditions.join(" AND ");

        Some(format!(
            "SELECT i.issue_id, CAST(i.number AS BIGINT) AS number, t.key AS team_key, i.title, \
             s.name AS status_name, s.category AS status_category, \
             i.priority, \
             ts_headline('english', c.body, query, \
             'MaxWords=35, MinWords=15, StartSel=**, StopSel=**') AS snippet, \
             'comment' AS match_field, \
             ts_rank(c.search_vector, query)::FLOAT8 AS rank \
             FROM comments c \
             JOIN issues i ON i.issue_id = c.issue_id \
             JOIN teams t ON t.team_id = i.team_id \
             JOIN statuses s ON s.status_id = i.status_id \
             CROSS JOIN plainto_tsquery('english', $1) query \
             WHERE {comment_where}"
        ))
    } else {
        None
    };

    let mut parts = Vec::new();
    if let Some(id_sql) = &identifier_sql {
        parts.push(id_sql.as_str());
    }
    parts.push(&issue_sql);
    if let Some(c_sql) = &comment_sql {
        parts.push(c_sql.as_str());
    }

    let sql = format!(
        "{} ORDER BY rank DESC LIMIT {} OFFSET {}",
        parts.join(" UNION ALL "),
        params.limit,
        params.offset
    );

    let rows: Vec<SearchResultRow> = trakkt_core::db_with_pool!(db, |p| {
        let mut query = sqlx::query_as::<_, SearchResultRow>(&sql);
        query = query.bind(&params.query);
        query = query.bind(&params.workspace_id);
        if let Some(team_id) = &params.team_id {
            query = query.bind(team_id);
        }
        if let Some((parsed_key, number)) = identifier {
            query = query.bind(number);
            if let Some(key) = parsed_key {
                query = query.bind(key.to_ascii_uppercase());
            }
        }
        query.fetch_all(p).await
    })?;

    Ok(rows)
}

// ─── SQLite implementation ──────────────────────────────────────────────────

async fn search_sqlite(
    db: &DbPool,
    params: &SearchParams,
    identifier: &Option<(Option<String>, i64)>,
) -> trakkt_core::Result<Vec<SearchResultRow>> {
    let pattern = escape_like_pattern(&params.query);

    // Bind positions:
    //   $1 = workspace_id
    //   $2 = LIKE pattern
    //   $3 = team_id (when team filter is active)
    //   $N = identifier number (when identifier detected)
    //   $N+1 = parsed team_key (when identifier has "KEY-NUM" format)

    let mut next_param = if params.team_id.is_some() { 4 } else { 3 };

    let mut issue_conditions = shared_conditions_sqlite(params);
    issue_conditions
        .push("(i.title LIKE $2 ESCAPE '\\' OR i.description LIKE $2 ESCAPE '\\')".to_string());
    let issue_where = issue_conditions.join(" AND ");

    let issue_sql = format!(
        "SELECT i.issue_id, CAST(i.number AS BIGINT) AS number, t.key AS team_key, i.title, \
         s.name AS status_name, s.category AS status_category, \
         i.priority, \
         NULL AS snippet, \
         CASE \
           WHEN i.title LIKE $2 ESCAPE '\\' THEN 'title' \
           WHEN i.description LIKE $2 ESCAPE '\\' THEN 'description' \
         END AS match_field, \
         0.0 AS rank \
         FROM issues i \
         JOIN teams t ON t.team_id = i.team_id \
         JOIN statuses s ON s.status_id = i.status_id \
         WHERE {issue_where}"
    );

    // Build the identifier UNION arm when the query looks like an identifier.
    let identifier_sql = if let Some((parsed_key, _number)) = identifier {
        let number_param = format!("${next_param}");
        next_param += 1;

        let mut id_conditions = shared_conditions_sqlite(params);
        id_conditions.push(format!("i.number = {number_param}"));

        if parsed_key.is_some() {
            let key_param = format!("${next_param}");
            id_conditions.push(format!("UPPER(t.key) = {key_param}"));
        }

        let id_where = id_conditions.join(" AND ");
        Some(format!(
            "SELECT i.issue_id, CAST(i.number AS BIGINT) AS number, t.key AS team_key, i.title, \
             s.name AS status_name, s.category AS status_category, \
             i.priority, \
             NULL AS snippet, \
             'identifier' AS match_field, \
             2.0 AS rank \
             FROM issues i \
             JOIN teams t ON t.team_id = i.team_id \
             JOIN statuses s ON s.status_id = i.status_id \
             WHERE {id_where}"
        ))
    } else {
        None
    };

    let comment_sql = if params.include_comments {
        let mut comment_conditions = shared_conditions_sqlite(params);
        comment_conditions.push("c.body LIKE $2 ESCAPE '\\'".to_string());
        let comment_where = comment_conditions.join(" AND ");

        Some(format!(
            "SELECT i.issue_id, CAST(i.number AS BIGINT) AS number, t.key AS team_key, i.title, \
             s.name AS status_name, s.category AS status_category, \
             i.priority, \
             NULL AS snippet, \
             'comment' AS match_field, \
             0.0 AS rank \
             FROM comments c \
             JOIN issues i ON i.issue_id = c.issue_id \
             JOIN teams t ON t.team_id = i.team_id \
             JOIN statuses s ON s.status_id = i.status_id \
             WHERE {comment_where}"
        ))
    } else {
        None
    };

    let mut parts = Vec::new();
    if let Some(id_sql) = &identifier_sql {
        parts.push(id_sql.as_str());
    }
    parts.push(&issue_sql);
    if let Some(c_sql) = &comment_sql {
        parts.push(c_sql.as_str());
    }

    let sql = format!(
        "{} ORDER BY rank DESC LIMIT {} OFFSET {}",
        parts.join(" UNION ALL "),
        params.limit,
        params.offset
    );

    let rows: Vec<SearchResultRow> = trakkt_core::db_with_pool!(db, |p| {
        let mut query = sqlx::query_as::<_, SearchResultRow>(&sql);
        query = query.bind(&params.workspace_id);
        query = query.bind(&pattern);
        if let Some(team_id) = &params.team_id {
            query = query.bind(team_id);
        }
        if let Some((parsed_key, number)) = identifier {
            query = query.bind(number);
            if let Some(key) = parsed_key {
                query = query.bind(key.to_ascii_uppercase());
            }
        }
        query.fetch_all(p).await
    })?;

    Ok(rows)
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn default_params() -> SearchParams {
        SearchParams {
            query: "test".to_string(),
            workspace_id: "ws-1".to_string(),
            team_id: None,
            include_archived: false,
            include_closed: false,
            include_comments: true,
            limit: 20,
            offset: 0,
        }
    }

    #[test]
    fn escape_like_pattern_basic() {
        assert_eq!(escape_like_pattern("hello"), "%hello%");
    }

    #[test]
    fn escape_like_pattern_special_chars() {
        assert_eq!(escape_like_pattern("100%"), "%100\\%%");
        assert_eq!(escape_like_pattern("a_b"), "%a\\_b%");
        assert_eq!(escape_like_pattern("c:\\path"), "%c:\\\\path%");
    }

    #[test]
    fn escape_like_pattern_combined() {
        assert_eq!(escape_like_pattern("a%b_c\\d"), "%a\\%b\\_c\\\\d%");
    }

    #[test]
    fn pg_conditions_default() {
        let params = default_params();
        let conds = shared_conditions_pg(&params);
        assert_eq!(conds, vec![
            "i.workspace_id = $2",
            "s.category NOT IN ('completed', 'cancelled')",
            "i.archived_at IS NULL",
        ]);
    }

    #[test]
    fn pg_conditions_with_team() {
        let mut params = default_params();
        params.team_id = Some("team-1".to_string());
        let conds = shared_conditions_pg(&params);
        assert!(conds.contains(&"i.team_id = $3".to_string()));
    }

    #[test]
    fn pg_conditions_include_closed() {
        let mut params = default_params();
        params.include_closed = true;
        let conds = shared_conditions_pg(&params);
        assert!(!conds.iter().any(|c| c.contains("completed")));
    }

    #[test]
    fn pg_conditions_include_archived() {
        let mut params = default_params();
        params.include_archived = true;
        let conds = shared_conditions_pg(&params);
        assert!(!conds.iter().any(|c| c.contains("archived_at")));
    }

    #[test]
    fn sqlite_conditions_default() {
        let params = default_params();
        let conds = shared_conditions_sqlite(&params);
        assert_eq!(conds, vec![
            "i.workspace_id = $1",
            "s.category NOT IN ('completed', 'cancelled')",
            "i.archived_at IS NULL",
        ]);
    }

    #[test]
    fn sqlite_conditions_with_team() {
        let mut params = default_params();
        params.team_id = Some("team-1".to_string());
        let conds = shared_conditions_sqlite(&params);
        assert!(conds.contains(&"i.team_id = $3".to_string()));
    }

    #[test]
    fn parse_identifier_full_key() {
        assert_eq!(
            parse_identifier_number("TRA-216"),
            Some((Some("TRA".to_string()), 216))
        );
    }

    #[test]
    fn parse_identifier_lowercase() {
        assert_eq!(
            parse_identifier_number("tra-42"),
            Some((Some("TRA".to_string()), 42))
        );
    }

    #[test]
    fn parse_identifier_mixed_case() {
        assert_eq!(
            parse_identifier_number("Tra-7"),
            Some((Some("TRA".to_string()), 7))
        );
    }

    #[test]
    fn parse_identifier_bare_number() {
        assert_eq!(parse_identifier_number("216"), Some((None, 216)));
    }

    #[test]
    fn parse_identifier_bare_number_with_whitespace() {
        assert_eq!(parse_identifier_number("  42  "), Some((None, 42)));
    }

    #[test]
    fn parse_identifier_plain_text() {
        assert_eq!(parse_identifier_number("hello"), None);
    }

    #[test]
    fn parse_identifier_partial_number() {
        // "21" is a bare number, should match as number 21 (not substring)
        assert_eq!(parse_identifier_number("21"), Some((None, 21)));
    }

    #[test]
    fn parse_identifier_empty() {
        assert_eq!(parse_identifier_number(""), None);
        assert_eq!(parse_identifier_number("  "), None);
    }

    #[test]
    fn parse_identifier_invalid_formats() {
        // Letters after dash
        assert_eq!(parse_identifier_number("TRA-abc"), None);
        // Number before dash
        assert_eq!(parse_identifier_number("123-456"), None);
        // Just a dash
        assert_eq!(parse_identifier_number("-"), None);
        // Prefix only
        assert_eq!(parse_identifier_number("TRA-"), None);
        // Suffix only
        assert_eq!(parse_identifier_number("-216"), None);
        // Zero issue number
        assert_eq!(parse_identifier_number("TRA-0"), None);
    }

    #[test]
    fn pg_and_sqlite_conditions_stay_in_sync() {
        let mut params = default_params();
        params.team_id = Some("team-1".to_string());
        params.include_closed = true;
        params.include_archived = true;

        let pg = shared_conditions_pg(&params);
        let sqlite = shared_conditions_sqlite(&params);

        assert_eq!(pg.len(), sqlite.len());
        for (p, s) in pg.iter().zip(sqlite.iter()) {
            let p_normalized = p.replace("$2", "$WS");
            let s_normalized = s.replace("$1", "$WS");
            assert_eq!(p_normalized, s_normalized);
        }
    }
}
