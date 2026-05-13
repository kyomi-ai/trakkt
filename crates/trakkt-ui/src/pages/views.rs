// SPDX-License-Identifier: AGPL-3.0-or-later

//! View filter types shared across the UI (e.g. issue list tab bar).

use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────────────────
// Filter/display types for JSON deserialization
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewFilters {
    #[serde(default)]
    pub statuses: Vec<String>,
    #[serde(default)]
    pub priorities: Vec<i32>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub search: String,
    #[serde(default)]
    pub team_id: String,
}
