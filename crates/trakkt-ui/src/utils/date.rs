// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared date formatting helpers used across multiple pages (issues, projects).

/// Formats a datetime string into a short date like "May 8".
/// Expects ISO 8601 format (e.g. "2026-05-08T..."). Falls back to the
/// first 10 characters if parsing fails.
pub fn format_short_date(datetime: &str) -> String {
    let date_part = if datetime.len() >= 10 {
        &datetime[..10]
    } else {
        datetime
    };
    let parts: Vec<&str> = date_part.split('-').collect();
    if parts.len() == 3 {
        let month = match parts[1] {
            "01" => "Jan",
            "02" => "Feb",
            "03" => "Mar",
            "04" => "Apr",
            "05" => "May",
            "06" => "Jun",
            "07" => "Jul",
            "08" => "Aug",
            "09" => "Sep",
            "10" => "Oct",
            "11" => "Nov",
            "12" => "Dec",
            _ => return date_part.to_string(),
        };
        // Strip leading zero from the day.
        let day = parts[2].trim_start_matches('0');
        format!("{month} {day}")
    } else {
        date_part.to_string()
    }
}

/// Formats an RFC 3339 date using JS `Intl.DateTimeFormat` on the client
/// (locale-aware) and a plain ISO → "May 8, 2026" on the server.
pub fn format_date_locale(date_str: &str) -> String {
    #[cfg(target_arch = "wasm32")]
    {
        use js_sys::{Date, Intl, Object, Reflect};
        use wasm_bindgen::JsValue;

        let date = Date::new(&JsValue::from_str(date_str));
        if date.get_time().is_nan() {
            return "Unknown".to_string();
        }

        let options = Object::new();
        let _ = Reflect::set(&options, &"month".into(), &"short".into());
        let _ = Reflect::set(&options, &"day".into(), &"numeric".into());
        let _ = Reflect::set(&options, &"year".into(), &"numeric".into());

        let locale = js_sys::Array::of1(&"en-US".into());
        let formatter = Intl::DateTimeFormat::new(&locale, &options);
        let format_fn = formatter.format();
        format_fn
            .call1(&JsValue::NULL, &date)
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_else(|| "Unknown".to_string())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        format_date(date_str)
    }
}

/// Formats an optional date string (YYYY-MM-DD or ISO 8601) into "May 8, 2026".
pub fn format_date(date: &str) -> String {
    let date_part = if date.len() >= 10 {
        &date[..10]
    } else {
        date
    };
    let parts: Vec<&str> = date_part.split('-').collect();
    if parts.len() == 3 {
        let month = match parts[1] {
            "01" => "Jan",
            "02" => "Feb",
            "03" => "Mar",
            "04" => "Apr",
            "05" => "May",
            "06" => "Jun",
            "07" => "Jul",
            "08" => "Aug",
            "09" => "Sep",
            "10" => "Oct",
            "11" => "Nov",
            "12" => "Dec",
            _ => return date_part.to_string(),
        };
        let day = parts[2].trim_start_matches('0');
        let year = parts[0];
        format!("{month} {day}, {year}")
    } else {
        date_part.to_string()
    }
}
