//! Wall-clock bucket keys without a time library: core stays dependency-free.

/// Days since 1970-01-01 to a proleptic Gregorian (year, month, day).
/// Howard Hinnant's `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// `(year, month, day, hour, minute)` of `ts_ms` on a clock `offset_min` east of UTC.
fn parts(ts_ms: i64, offset_min: i32) -> (i64, u32, u32, u32, u32) {
    let local_min = ts_ms.div_euclid(60_000) + i64::from(offset_min);
    let days = local_min.div_euclid(1_440);
    let in_day = local_min.rem_euclid(1_440);
    let (year, month, day) = civil_from_days(days);
    (year, month, day, (in_day / 60) as u32, (in_day % 60) as u32)
}

/// `YYYY-MM-DDTHH:MM` of the `step_min`-minute bucket holding `ts_ms`
/// (`YYYY-MM-DDTHH` when `step_min` is 60).
pub fn bucket_key(ts_ms: i64, offset_min: i32, step_min: u32) -> String {
    let (year, month, day, hour, minute) = parts(ts_ms, offset_min);
    if step_min >= 60 {
        return format!("{year:04}-{month:02}-{day:02}T{hour:02}");
    }
    let minute = minute - minute % step_min.max(1);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}")
}

/// `YYYY-MM-DD` of `ts_ms` on a clock `offset_min` east of UTC.
pub fn day_key(ts_ms: i64, offset_min: i32) -> String {
    let (year, month, day, _, _) = parts(ts_ms, offset_min);
    format!("{year:04}-{month:02}-{day:02}")
}

/// The start of the `step_min`-minute bucket holding `ts_ms`, in UTC milliseconds.
pub(crate) fn bucket_start(ts_ms: i64, offset_min: i32, step_min: u32) -> i64 {
    let step = i64::from(step_min.max(1)) * 60_000;
    let offset = i64::from(offset_min) * 60_000;
    (ts_ms + offset).div_euclid(step) * step - offset
}
