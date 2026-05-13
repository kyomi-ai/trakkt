// SPDX-License-Identifier: AGPL-3.0-or-later

//! Relative timestamp formatting shared across pages (issue detail, inbox).

/// Convert an ISO timestamp string to a human-friendly relative format.
///
/// Returns: "just now", "2m ago", "1h ago", "3d ago", "May 5", "Dec 15, 2025"
///
/// Uses `js_sys::Date::now()` for current time (WASM-safe, no `wasmbind` chrono feature needed).
/// Falls back to the raw string if parsing fails.
pub fn relative_time(timestamp: &str) -> String {
    use chrono::NaiveDateTime;

    let parsed = timestamp
        .parse::<chrono::DateTime<chrono::Utc>>()
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .or_else(|| NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M:%S").ok())
                .map(|naive| naive.and_utc())
        });

    let ts = match parsed {
        Some(dt) => dt,
        None => return timestamp.to_string(),
    };

    let now_ms = js_sys::Date::now();
    let now_secs = (now_ms / 1000.0) as i64;
    let ts_secs = ts.timestamp();
    let diff_secs = now_secs - ts_secs;

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

    let now_year = {
        let d = js_sys::Date::new_0();
        d.get_full_year() as i32
    };

    let ts_year = ts.format("%Y").to_string().parse::<i32>().unwrap_or(0);
    if ts_year == now_year {
        ts.format("%b %-d").to_string()
    } else {
        ts.format("%b %-d, %Y").to_string()
    }
}
