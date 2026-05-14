// SPDX-License-Identifier: AGPL-3.0-or-later

//! Relative timestamp formatting shared across pages (issue detail, inbox).

use chrono::{DateTime, Datelike, NaiveDateTime, Utc};

/// Format a `DateTime<Utc>` as a human-friendly relative string.
///
/// Returns: "just now", "2m ago", "1h ago", "3d ago", "May 5", "Dec 15, 2025"
pub fn format_datetime(dt: &DateTime<Utc>) -> String {
    let now = Utc::now();
    let diff = now.signed_duration_since(*dt);
    let diff_secs = diff.num_seconds();

    if diff_secs < 60 {
        return "just now".to_string();
    }

    let diff_mins = diff_secs / 60;
    if diff_mins < 60 {
        return format!("{diff_mins}m ago");
    }

    let diff_hours = diff_mins / 60;
    if diff_hours < 24 {
        return format!("{diff_hours}h ago");
    }

    let diff_days = diff_hours / 24;
    if diff_days < 7 {
        return format!("{diff_days}d ago");
    }

    if dt.year() == now.year() {
        dt.format("%b %-d").to_string()
    } else {
        dt.format("%b %-d, %Y").to_string()
    }
}

/// Convert an ISO timestamp string to a human-friendly relative format.
///
/// Returns: "just now", "2m ago", "1h ago", "3d ago", "May 5", "Dec 15, 2025"
///
/// Parses multiple timestamp formats, then delegates to [`format_datetime`].
/// Falls back to the raw string if parsing fails.
pub fn relative_time(timestamp: &str) -> String {
    let parsed = timestamp
        .parse::<DateTime<Utc>>()
        .ok()
        .or_else(|| {
            DateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M:%S%.f%#z")
                .ok()
                .map(|dt| dt.to_utc())
        })
        .or_else(|| {
            NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .or_else(|| NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M:%S").ok())
                .map(|naive| naive.and_utc())
        });

    match parsed {
        Some(dt) => format_datetime(&dt),
        None => timestamp.to_string(),
    }
}
