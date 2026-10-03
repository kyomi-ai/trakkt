// SPDX-License-Identifier: AGPL-3.0-or-later

pub mod archived;
pub mod filters;
pub mod issue_detail;
pub mod issue_list;
pub mod issue_row;
pub mod my_issues;
pub mod workspace_view;

use trakkt_types::models::{IssueWithDetails, Team};

/// Default number of days after which completed/cancelled issues are considered archived.
///
/// Used as a fallback when neither team nor workspace settings specify
/// an auto-archive duration. Resolution order:
/// 1. Team's own `auto_archive_days` (if `Some` and > 0)
/// 2. Workspace-level `default_auto_archive_days` (if `Some` and > 0)
/// 3. This constant
pub const DEFAULT_ARCHIVE_DAYS: u32 = 30;

/// Returns `true` if an issue should be considered "archived" — i.e. it has a
/// completed or cancelled status category and its `updated_at` timestamp is
/// older than `archive_days` days ago.
///
/// On WASM, uses `js_sys::Date` for current time and date parsing.
/// On SSR, always returns `false` (no client-side filtering on server render).
#[cfg(target_arch = "wasm32")]
pub fn is_archived(issue: &IssueWithDetails, archive_days: u32) -> bool {
    if issue.status_category != "completed" && issue.status_category != "cancelled" {
        return false;
    }
    let now = js_sys::Date::now(); // ms since epoch
    let updated = js_sys::Date::parse(&issue.updated_at); // ms since epoch
    if updated.is_nan() {
        return false;
    }
    let age_ms = now - updated;
    let threshold_ms = archive_days as f64 * 24.0 * 60.0 * 60.0 * 1000.0;
    age_ms > threshold_ms
}

#[cfg(not(target_arch = "wasm32"))]
pub fn is_archived(_issue: &IssueWithDetails, _archive_days: u32) -> bool {
    false
}

/// Resolve the effective archive-days using a three-tier cascade:
///
/// 1. **Team setting** — `team.settings.auto_archive_days`:
///    `Some(0)` means archiving is explicitly disabled for this team,
///    `Some(N)` where N > 0 is an explicit per-team override.
/// 2. **Workspace default** — `workspace_default`:
///    The workspace-level `default_auto_archive_days` setting, if configured
///    and > 0.
/// 3. **Compile-time fallback** — [`DEFAULT_ARCHIVE_DAYS`] (30 days).
///
/// Returns `0` when archiving is disabled (issues should not be filtered).
pub fn resolve_archive_days(team: Option<&Team>, workspace_default: Option<u32>) -> u32 {
    // Step 1: team explicit setting (Some(0) = disabled, Some(N) = N days)
    if let Some(team) = team
        && let Some(days) = team.settings.as_ref().and_then(|s| s.auto_archive_days)
    {
        return days;
    }
    // Step 2: workspace default
    if let Some(days) = workspace_default
        && days > 0
    {
        return days;
    }
    // Step 3: compile-time fallback
    DEFAULT_ARCHIVE_DAYS
}
