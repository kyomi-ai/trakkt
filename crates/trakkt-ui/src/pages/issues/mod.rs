// SPDX-License-Identifier: AGPL-3.0-or-later

pub mod filters;
pub mod issue_detail;
pub mod issue_list;
pub mod issue_row;
pub mod my_issues;

use trakkt_types::models::IssueWithDetails;

/// Default number of days after which completed/cancelled issues are considered archived.
pub const ARCHIVE_DAYS: u32 = 30;

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
