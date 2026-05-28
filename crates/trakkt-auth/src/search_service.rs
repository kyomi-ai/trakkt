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
pub async fn search(
    db: &DbPool,
    params: &SearchParams,
) -> trakkt_core::Result<Vec<SearchResult>> {
    if params.query.trim().is_empty() {
        return Ok(Vec::new());
    }

    let rows = if db.is_postgres() {
        search_postgres(db, params).await?
    } else {
        search_sqlite(db, params).await?
    };

    Ok(rows.into_iter().map(SearchResult::from).collect())
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
) -> trakkt_core::Result<Vec<SearchResultRow>> {
    // $1 = search query text, $2 = workspace_id, $3 = team_id (when present).

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

    let sql = if params.include_comments {
        let mut comment_conditions = shared_conditions_pg(params);
        comment_conditions.push("c.search_vector @@ query".to_string());
        let comment_where = comment_conditions.join(" AND ");

        let comment_sql = format!(
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
        );

        format!(
            "{issue_sql} UNION ALL {comment_sql} ORDER BY rank DESC LIMIT {} OFFSET {}",
            params.limit, params.offset
        )
    } else {
        format!(
            "{issue_sql} ORDER BY rank DESC LIMIT {} OFFSET {}",
            params.limit, params.offset
        )
    };

    let rows: Vec<SearchResultRow> = trakkt_core::db_with_pool!(db, |p| {
        let mut query = sqlx::query_as::<_, SearchResultRow>(&sql);
        query = query.bind(&params.query);
        query = query.bind(&params.workspace_id);
        if let Some(ref team_id) = params.team_id {
            query = query.bind(team_id);
        }
        query.fetch_all(p).await
    })?;

    Ok(rows)
}

// ─── SQLite implementation ──────────────────────────────────────────────────

async fn search_sqlite(
    db: &DbPool,
    params: &SearchParams,
) -> trakkt_core::Result<Vec<SearchResultRow>> {
    let pattern = escape_like_pattern(&params.query);

    // $1 = workspace_id, $2 = LIKE pattern, $3 = team_id (when present).

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

    let sql = if params.include_comments {
        let mut comment_conditions = shared_conditions_sqlite(params);
        comment_conditions.push("c.body LIKE $2 ESCAPE '\\'".to_string());
        let comment_where = comment_conditions.join(" AND ");

        let comment_sql = format!(
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
        );

        format!(
            "{issue_sql} UNION ALL {comment_sql} LIMIT {} OFFSET {}",
            params.limit, params.offset
        )
    } else {
        format!(
            "{issue_sql} LIMIT {} OFFSET {}",
            params.limit, params.offset
        )
    };

    let rows: Vec<SearchResultRow> = trakkt_core::db_with_pool!(db, |p| {
        let mut query = sqlx::query_as::<_, SearchResultRow>(&sql);
        query = query.bind(&params.workspace_id);
        query = query.bind(&pattern);
        if let Some(ref team_id) = params.team_id {
            query = query.bind(team_id);
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
