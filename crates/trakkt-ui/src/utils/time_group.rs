// SPDX-License-Identifier: AGPL-3.0-or-later

//! Time grouping utilities — classify timestamps into Today, Yesterday,
//! This Week, or Older buckets for chronological feed UIs.

#[derive(Clone, Copy, PartialEq)]
pub enum TimeGroup {
    Today,
    Yesterday,
    ThisWeek,
    Older,
}

impl TimeGroup {
    pub fn label(self) -> &'static str {
        match self {
            Self::Today => "Today",
            Self::Yesterday => "Yesterday",
            Self::ThisWeek => "This Week",
            Self::Older => "Older",
        }
    }
}

pub fn classify_time_group(created_at: &str) -> TimeGroup {
    use chrono::NaiveDateTime;

    let parsed = created_at
        .parse::<chrono::DateTime<chrono::Utc>>()
        .ok()
        .or_else(|| {
            chrono::DateTime::parse_from_str(created_at, "%Y-%m-%d %H:%M:%S%.f%#z")
                .ok()
                .map(|dt| dt.to_utc())
        })
        .or_else(|| {
            NaiveDateTime::parse_from_str(created_at, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .or_else(|| {
                    NaiveDateTime::parse_from_str(created_at, "%Y-%m-%d %H:%M:%S").ok()
                })
                .map(|naive| naive.and_utc())
        });

    let ts = match parsed {
        Some(dt) => dt,
        None => return TimeGroup::Older,
    };

    let ts_ms = ts.timestamp_millis() as f64;

    let now = js_sys::Date::new_0();
    let now_day = now.get_day() as i32;

    let today_start = js_sys::Date::new_0();
    today_start.set_hours(0);
    today_start.set_minutes(0);
    today_start.set_seconds(0);
    today_start.set_milliseconds(0);
    let today_start_ms = today_start.get_time();

    if ts_ms >= today_start_ms {
        return TimeGroup::Today;
    }

    let yesterday_start_ms = today_start_ms - 86_400_000.0;
    if ts_ms >= yesterday_start_ms {
        return TimeGroup::Yesterday;
    }

    let monday_offset = if now_day == 0 { 6 } else { now_day - 1 };
    let week_start_ms = today_start_ms - (monday_offset as f64 * 86_400_000.0);
    if ts_ms >= week_start_ms {
        return TimeGroup::ThisWeek;
    }

    TimeGroup::Older
}
