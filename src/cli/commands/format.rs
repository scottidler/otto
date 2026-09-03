//! Display formatters shared by `Clean`, `History`, and `Stats`.
//!
//! These three commands render the same run metadata - a byte count, a
//! duration, a Unix timestamp - and each had grown its own copy. The copies had
//! drifted: `Clean` printed gigabytes to one decimal place where `History` and
//! `Stats` printed two, and `Clean` rendered timestamps in UTC where the other
//! two used local time, so the same run showed two different times depending on
//! which command you asked. One implementation each, local time throughout.

use chrono::{DateTime, Local, TimeZone, Utc};

/// A byte count in the largest unit that keeps the number readable.
pub fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// A duration in seconds, in the largest unit that keeps the number readable.
pub fn format_duration(seconds: f64) -> String {
    if seconds < 1.0 {
        format!("{:.0}ms", seconds * 1000.0)
    } else if seconds < 60.0 {
        format!("{seconds:.1}s")
    } else if seconds < 3600.0 {
        let minutes = (seconds / 60.0) as u64;
        let secs = (seconds % 60.0) as u64;
        format!("{minutes}m{secs}s")
    } else {
        let hours = (seconds / 3600.0) as u64;
        let minutes = ((seconds % 3600.0) / 60.0) as u64;
        format!("{hours}h{minutes}m")
    }
}

/// A Unix timestamp as local wall-clock time.
///
/// An out-of-range or ambiguous local timestamp falls back to the epoch rather
/// than panicking: this is cosmetic display, and a run listing is not worth
/// aborting over an unrepresentable date.
pub fn format_timestamp(timestamp: u64) -> String {
    let dt = Local
        .timestamp_opt(timestamp as i64, 0)
        .single()
        .unwrap_or_else(|| DateTime::<Local>::from(DateTime::<Utc>::MIN_UTC));
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

#[path = "format_tests.rs"]
mod tests;
