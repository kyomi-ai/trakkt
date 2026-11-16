// SPDX-License-Identifier: AGPL-3.0-or-later

//! Search service — full-text search across issues and comments.
//!
//! On Postgres, uses `tsvector` columns with GIN indexes for ranked full-text
//! search with snippet highlighting. On SQLite, falls back to `LIKE` matching
//! without ranking or snippets.

use trakkt_core::DbPool;
use trakkt_types::api::FilterClause;

use crate::filter_sql::{clauses_to_sql, SqlArg};

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
    /// Number of distinct matching issues, counted up to [`SEARCH_FETCH_CAP`]
    /// rows. When `truncated` is true this is a lower bound.
    pub total: i64,
    /// More results can be fetched by repeating the request with
    /// `offset = next_offset`.
    pub has_more: bool,
    /// Offset of the next page; `None` when `has_more` is false.
    pub next_offset: Option<i64>,
    /// The search hit its internal fetch cap: matches ranked below the cap are
    /// not reachable by paging and `total` is a lower bound. Narrow the query.
    pub truncated: bool,
}

/// Most rows one search will fetch before deduplicating and paging.
pub const SEARCH_FETCH_CAP: i64 = 1000;

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
    /// Exclude archived issues. Only set when the caller asked for it: search
    /// applies exactly the filters the user set and nothing implicit.
    pub exclude_archived: bool,
    /// Exclude completed and cancelled issues. Only set when the caller asked
    /// for it (e.g. legacy `include_closed=false`).
    pub exclude_closed: bool,
    /// Restrict to one status ID (same meaning as the issue-list filter).
    pub status_id: Option<String>,
    /// Restrict to these status categories (same meaning as the issue-list filter).
    pub status_categories: Option<Vec<String>>,
    /// Composable issue-list filter clauses (status, priority, label,
    /// project, relations, ...), applied in SQL before ranking and paging.
    pub clauses: Vec<FilterClause>,
    pub include_comments: bool,
    pub limit: i64,
    pub offset: i64,
}

/// Everything the caller's filters add to the query: the shared WHERE
/// conditions and the values bound after the team ID, in placeholder order.
struct FilterPlan {
    conditions: Vec<String>,
    args: Vec<SqlArg>,
    /// First placeholder index not used by the workspace/query/team/filters.
    next_param: usize,
}

impl SearchParams {
    /// Build the filter conditions. Only filters the caller set appear.
    fn plan(&self) -> FilterPlan {
        let mut conditions = Vec::new();
        let mut args = Vec::new();
        let mut next = 3;
        if self.team_id.is_some() {
            conditions.push("i.team_id = $3".to_string());
            next = 4;
        }
        if let Some(id) = &self.status_id {
            conditions.push(format!("i.status_id = ${next}"));
            args.push(SqlArg::Text(id.clone()));
            next += 1;
        }
        if let Some(categories) = &self.status_categories
            && !categories.is_empty()
        {
            let (in_clause, after) =
                trakkt_core::db::in_clause_placeholders(categories.len(), next);
            conditions.push(format!("s.category IN {in_clause}"));
            args.extend(categories.iter().cloned().map(SqlArg::Text));
            next = after;
        }
        let compiled = clauses_to_sql(&self.clauses, next);
        conditions.extend(compiled.conditions);
        args.extend(compiled.args);
        next = compiled.next_param;
        if self.exclude_closed {
            conditions.push("s.category NOT IN ('completed', 'cancelled')".to_string());
        }
        if self.exclude_archived {
            conditions.push("i.archived_at IS NULL".to_string());
        }
        FilterPlan {
            conditions,
            args,
            next_param: next,
        }
    }
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
            has_more: false,
            next_offset: None,
            truncated: false,
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
        exclude_archived: params.exclude_archived,
        exclude_closed: params.exclude_closed,
        status_id: params.status_id.clone(),
        status_categories: params.status_categories.clone(),
        clauses: params.clauses.clone(),
        include_comments: params.include_comments,
        // One past the cap, so a result set of exactly the cap is not
        // mistaken for a truncated one.
        limit: SEARCH_FETCH_CAP + 1,
        offset: 0,
    };

    let mut rows = if db.is_postgres() {
        search_postgres(db, &uncapped_params, &identifier).await?
    } else {
        search_sqlite(db, &uncapped_params, &identifier).await?
    };

    let truncated = rows.len() as i64 > SEARCH_FETCH_CAP;
    rows.truncate(SEARCH_FETCH_CAP as usize);

    // The SQL already keeps one best-ranked row per issue (identifier matches
    // rank 2.0 so they win over tsvector / LIKE matches), so the cap counts
    // distinct issues and comment rows cannot crowd out other hits.
    let all_results: Vec<SearchResult> = rows.into_iter().map(SearchResult::from).collect();

    let total = all_results.len() as i64;

    // Apply pagination in Rust.
    let offset = params.offset as usize;
    let limit = params.limit as usize;
    let results: Vec<SearchResult> = all_results.into_iter().skip(offset).take(limit).collect();

    let next = (offset + results.len()) as i64;
    let has_more = next < total;

    Ok(SearchResponse {
        results,
        total,
        has_more,
        next_offset: has_more.then_some(next),
        truncated,
    })
}

// ─── Shared condition builders ─────────────────────────────────────────────

/// Build the WHERE conditions shared between issue and comment arms (Postgres).
/// Workspace is always `$2`, team_id is `$3` when present.
fn shared_conditions_pg(params: &SearchParams) -> Vec<String> {
    shared_conditions("i.workspace_id = $2", params)
}

/// Build the WHERE conditions shared between issue and comment arms (SQLite).
/// Workspace is always `$1`, team_id is `$3` when present.
fn shared_conditions_sqlite(params: &SearchParams) -> Vec<String> {
    shared_conditions("i.workspace_id = $1", params)
}

/// Only filters the caller explicitly set become conditions.
fn shared_conditions(workspace_cond: &str, params: &SearchParams) -> Vec<String> {
    let mut conds = vec![workspace_cond.to_string()];
    conds.extend(params.plan().conditions);
    conds
}

/// Keep one best-ranked row per issue, then order and page. Applied in SQL so
/// the row cap counts distinct issues.
fn dedup_and_page(union_sql: &str, limit: i64, offset: i64) -> String {
    format!(
        "SELECT issue_id, number, team_key, title, status_name, status_category, \
         priority, snippet, match_field, rank FROM (\
         SELECT u.*, ROW_NUMBER() OVER (PARTITION BY u.issue_id \
         ORDER BY u.rank DESC, u.match_field ASC) AS rn FROM ({union_sql}) u) d \
         WHERE rn = 1 ORDER BY rank DESC, issue_id ASC, match_field ASC \
         LIMIT {limit} OFFSET {offset}"
    )
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

    let plan_args = params.plan().args;
    let mut next_param = params.plan().next_param;

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

    let sql = dedup_and_page(&parts.join(" UNION ALL "), params.limit, params.offset);

    let rows: Vec<SearchResultRow> = trakkt_core::db_with_pool!(db, |p| {
        let mut query = sqlx::query_as::<_, SearchResultRow>(&sql);
        query = query.bind(&params.query);
        query = query.bind(&params.workspace_id);
        if let Some(team_id) = &params.team_id {
            query = query.bind(team_id);
        }
        for arg in &plan_args {
            query = match arg {
                SqlArg::Text(v) => query.bind(v),
                SqlArg::Int(n) => query.bind(*n),
            };
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

    let plan_args = params.plan().args;
    let mut next_param = params.plan().next_param;

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

    let sql = dedup_and_page(&parts.join(" UNION ALL "), params.limit, params.offset);

    let rows: Vec<SearchResultRow> = trakkt_core::db_with_pool!(db, |p| {
        let mut query = sqlx::query_as::<_, SearchResultRow>(&sql);
        query = query.bind(&params.workspace_id);
        query = query.bind(&pattern);
        if let Some(team_id) = &params.team_id {
            query = query.bind(team_id);
        }
        for arg in &plan_args {
            query = match arg {
                SqlArg::Text(v) => query.bind(v),
                SqlArg::Int(n) => query.bind(*n),
            };
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
            exclude_archived: false,
            exclude_closed: false,
            status_id: None,
            status_categories: None,
            clauses: Vec::new(),
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
        // No hidden defaults: only the workspace scope applies.
        assert_eq!(conds, vec!["i.workspace_id = $2"]);
    }

    #[test]
    fn pg_conditions_with_team() {
        let mut params = default_params();
        params.team_id = Some("team-1".to_string());
        let conds = shared_conditions_pg(&params);
        assert!(conds.contains(&"i.team_id = $3".to_string()));
    }

    #[test]
    fn pg_conditions_explicit_exclusions() {
        let mut params = default_params();
        params.exclude_closed = true;
        params.exclude_archived = true;
        let conds = shared_conditions_pg(&params);
        assert!(conds.iter().any(|c| c.contains("NOT IN ('completed'")));
        assert!(conds.iter().any(|c| c.contains("archived_at IS NULL")));
    }

    #[test]
    fn status_filters_bind_after_team() {
        let mut params = default_params();
        params.team_id = Some("team-1".to_string());
        params.status_id = Some("s1".to_string());
        params.status_categories = Some(vec!["completed".to_string(), "cancelled".to_string()]);
        let conds = shared_conditions_pg(&params);
        assert!(conds.contains(&"i.status_id = $4".to_string()));
        assert!(conds.contains(&"s.category IN ($5, $6)".to_string()));
        let plan = params.plan();
        assert_eq!(
            plan.args,
            vec![
                SqlArg::Text("s1".into()),
                SqlArg::Text("completed".into()),
                SqlArg::Text("cancelled".into()),
            ]
        );
        assert_eq!(plan.next_param, 7);
    }

    #[test]
    fn clauses_bind_after_status_filters() {
        let mut params = default_params();
        params.team_id = Some("team-1".to_string());
        params.status_id = Some("s1".to_string());
        params.clauses = vec![FilterClause {
            field: "label".to_string(),
            operator: "any_of".to_string(),
            values: vec!["l1".to_string(), "l2".to_string()],
        }];
        let plan = params.plan();
        assert!(plan.conditions.iter().any(|c| c.contains("il.label_id IN ($5, $6)")));
        assert_eq!(plan.next_param, 7);
    }

    #[test]
    fn dedup_wraps_the_union_without_changing_its_columns() {
        let sql = dedup_and_page("SELECT 1", 10, 0);
        assert!(sql.contains("ROW_NUMBER() OVER (PARTITION BY u.issue_id"));
        assert!(sql.ends_with("LIMIT 10 OFFSET 0"));
    }

    #[test]
    fn sqlite_conditions_default() {
        let params = default_params();
        let conds = shared_conditions_sqlite(&params);
        assert_eq!(conds, vec!["i.workspace_id = $1"]);
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
        params.exclude_closed = true;
        params.exclude_archived = true;
        params.status_id = Some("s1".to_string());
        params.status_categories = Some(vec!["started".to_string()]);

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
