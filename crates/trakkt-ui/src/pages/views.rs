// SPDX-License-Identifier: AGPL-3.0-or-later

//! View filter types shared across the UI (e.g. issue list tab bar).

use serde::{Deserialize, Serialize};

// Re-export from the canonical definition in trakkt-types.
pub use trakkt_types::api::FilterClause;

/// New composable view filters — replaces the old flat field-per-filter struct.
///
/// Saved views serialize this to JSON. Backwards compatibility: the old format
/// is handled via a migration path in `issue_list.rs` tab click handlers.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewFilters {
    #[serde(default)]
    pub clauses: Vec<FilterClause>,
    /// Persisted sort field (e.g. "priority", "status", "created_date").
    #[serde(default)]
    pub sort_field: Option<String>,
    /// Persisted sort direction ("asc" or "desc").
    #[serde(default)]
    pub sort_direction: Option<String>,
}

// ───────────────────────────────────────────────���─────────────────────────────
// Legacy format — for deserializing old saved views
// ─────────────────────────────────────────────────────────────────────────────

/// The old flat filter format. Used to migrate saved views that were persisted
/// before TRA-103 introduced the composable clause model.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct LegacyViewFilters {
    #[serde(default)]
    pub statuses: Vec<String>,
    #[serde(default)]
    pub priorities: Vec<i32>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub project_ids: Vec<String>,
    #[serde(default)]
    pub search: String,
    #[serde(default)]
    pub team_id: String,
    #[serde(default)]
    pub sort_field: Option<String>,
    #[serde(default)]
    pub sort_direction: Option<String>,
}

impl LegacyViewFilters {
    /// Convert the legacy format into the new composable format.
    pub fn into_view_filters(self) -> ViewFilters {
        let mut clauses = Vec::new();
        if !self.statuses.is_empty() {
            clauses.push(FilterClause {
                field: "status".to_string(),
                operator: "any_of".to_string(),
                values: self.statuses,
            });
        }
        if !self.priorities.is_empty() {
            clauses.push(FilterClause {
                field: "priority".to_string(),
                operator: "any_of".to_string(),
                values: self.priorities.iter().map(|p| p.to_string()).collect(),
            });
        }
        if !self.labels.is_empty() {
            clauses.push(FilterClause {
                field: "label".to_string(),
                operator: "any_of".to_string(),
                values: self.labels,
            });
        }
        if !self.project_ids.is_empty() {
            clauses.push(FilterClause {
                field: "project".to_string(),
                operator: "any_of".to_string(),
                values: self.project_ids,
            });
        }
        ViewFilters {
            clauses,
            sort_field: self.sort_field,
            sort_direction: self.sort_direction,
        }
    }
}
