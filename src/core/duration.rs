//! Bounded positive durations used by authoring policies.

use crate::core::canonical::MAX_SAFE_INTEGER;

pub fn positive_nanoseconds(value: &str) -> Result<u64, String> {
    let (number, multiplier) = [
        ("ms", 1_000_000u64),
        ("s", 1_000_000_000),
        ("min", 60_000_000_000),
        ("h", 3_600_000_000_000),
    ]
    .into_iter()
    .find_map(|(suffix, multiplier)| {
        value
            .trim()
            .strip_suffix(suffix)
            .map(|n| (n.trim(), multiplier))
    })
    .ok_or("duration requires a positive integer and ms, s, min, or h")?;
    if !number.starts_with(|c: char| matches!(c, '1'..='9'))
        || !number.bytes().all(|c| c.is_ascii_digit())
    {
        return Err("duration requires a positive decimal integer".into());
    }
    number
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(multiplier))
        .filter(|n| *n > 0 && *n <= MAX_SAFE_INTEGER)
        .ok_or_else(|| "duration is zero, invalid, or too large".into())
}
