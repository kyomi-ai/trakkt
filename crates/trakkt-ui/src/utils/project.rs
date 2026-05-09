// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared project helpers — maps project status strings to badge variants
//! and display labels.

use crate::components::StatusBadgeVariant;

/// Maps a project status string to a StatusBadgeVariant for visual styling.
pub fn status_variant(status: &str) -> StatusBadgeVariant {
    match status {
        "in_progress" => StatusBadgeVariant::Info,
        "completed" => StatusBadgeVariant::Success,
        "paused" => StatusBadgeVariant::Warning,
        "cancelled" => StatusBadgeVariant::Error,
        // "planned" and anything else
        _ => StatusBadgeVariant::Default,
    }
}

/// Returns a human-readable label for a project status string.
pub fn status_label(status: &str) -> String {
    match status {
        "planned" => "Planned".to_string(),
        "in_progress" => "In Progress".to_string(),
        "paused" => "Paused".to_string(),
        "completed" => "Completed".to_string(),
        "cancelled" => "Cancelled".to_string(),
        other => other.to_string(),
    }
}
