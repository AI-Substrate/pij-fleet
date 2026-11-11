//! One test-only wall-clock scale (plan 209, F28).
//!
//! A test that times real child processes (`ps`, tmux, a shell) against a
//! wall-clock budget multiplies the budget by `PIJ_TEST_TIME_SCALE` (default 1),
//! so a loaded or niced machine stretches the budget instead of failing.
//! Production constants never read it. The TS driver reads the same variable
//! (`harness/driver/time-scale.ts`).
use std::time::Duration;

/// The variable that stretches every real-child test budget.
pub const TIME_SCALE_VAR: &str = "PIJ_TEST_TIME_SCALE";

/// The scale from `PIJ_TEST_TIME_SCALE`; 1 when unset or empty.
///
/// # Panics
/// On a value that is not a finite number of at least 1, so a typo can never
/// silently tighten or disable a budget.
pub fn time_scale() -> f64 {
    parse_time_scale(std::env::var(TIME_SCALE_VAR).ok().as_deref())
        .unwrap_or_else(|raw| panic!("{TIME_SCALE_VAR} must be a number >= 1, not {raw:?}"))
}

/// `budget` stretched by [`time_scale`].
pub fn scaled(budget: Duration) -> Duration {
    budget.mul_f64(time_scale())
}

/// Parses a raw value: `None` or blank is 1; otherwise a finite number >= 1,
/// or the raw text back as the error.
pub fn parse_time_scale(raw: Option<&str>) -> Result<f64, String> {
    let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(1.0);
    };
    match raw.parse::<f64>() {
        Ok(scale) if scale.is_finite() && scale >= 1.0 => Ok(scale),
        _ => Err(raw.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_time_scale;

    #[test]
    fn unset_or_blank_is_one_and_only_a_number_of_at_least_one_scales() {
        assert_eq!(parse_time_scale(None), Ok(1.0));
        assert_eq!(parse_time_scale(Some(" ")), Ok(1.0));
        assert_eq!(parse_time_scale(Some("4")), Ok(4.0));
        assert_eq!(parse_time_scale(Some("2.5")), Ok(2.5));
        for bad in ["0.5", "0", "-2", "fast", "inf", "NaN"] {
            assert_eq!(parse_time_scale(Some(bad)), Err(bad.to_string()), "{bad}");
        }
    }
}
