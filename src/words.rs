//! Numbers, sizes and times as a post says them.

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// A size: 812 KB, 1.4 GB.
pub fn size(bytes: u64) -> String {
    let b = bytes as f64;
    match bytes {
        0..=999 => format!("{bytes} B"),
        1_000..=999_999 => format!("{:.0} KB", b / 1e3),
        1_000_000..=999_999_999 => format!("{:.1} MB", b / 1e6),
        _ => format!("{:.2} GB", b / 1e9),
    }
}

/// A count with thousands marked: 12,345.
pub fn count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A day from an RFC 3339 time: 3 Oct 2026, 15:00 UTC with `clock`.
pub fn day(at: &str, clock: bool) -> String {
    let Ok(t) = OffsetDateTime::parse(at, &Rfc3339) else { return at.to_string() };
    let t = t.to_offset(time::UtcOffset::UTC);
    let month = &t.month().to_string()[..3];
    if clock {
        format!("{} {month} {} {:02}:{:02} UTC", t.day(), t.year(), t.hour(), t.minute())
    } else {
        format!("{} {month} {}", t.day(), t.year())
    }
}

/// A list in words: a, b and c.
pub fn and(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_them() {
        assert_eq!(size(812_000), "812 KB");
        assert_eq!(size(1_400_000_000), "1.40 GB");
        assert_eq!(count(1234567), "1,234,567");
        assert_eq!(count(999), "999");
        assert_eq!(day("2026-10-03T15:04:00Z", false), "3 Oct 2026");
        assert_eq!(day("2026-10-03T23:30:00-02:00", true), "4 Oct 2026 01:30 UTC");
        assert_eq!(and(&["title", "tags", "privacy"]), "title, tags and privacy");
    }
}
