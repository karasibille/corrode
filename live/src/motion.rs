//! The moment of a live animation: where the ranges of a recipe stand
//! (`a..b`, 0 to 1) at a time since it started.

use std::time::Duration;

use corrode_effects::Easing;

/// How long a trip from one end of the ranges to the other takes at the
/// start, and the shortest and longest it can be set to.
pub const PERIOD: Duration = Duration::from_millis(3000);
pub const SHORTEST: Duration = Duration::from_millis(300);
pub const LONGEST: Duration = Duration::from_secs(60);

/// Where the ranges stand after `elapsed`: from 0 to 1 over a `period`,
/// then back to 0 over the next, for ever, slow at both ends.
pub fn moment(elapsed: Duration, period: Duration) -> f64 {
    let trips = elapsed.as_secs_f64() / period.as_secs_f64().max(f64::EPSILON);
    let phase = trips % 2.0;
    let along = if phase < 1.0 { phase } else { 2.0 - phase };
    Easing::Smooth.at(along)
}

/// A period made longer or shorter by a factor, kept within bounds.
pub fn scaled(period: Duration, factor: f64) -> Duration {
    period.mul_f64(factor).clamp(SHORTEST, LONGEST)
}

/// The cycle of the patterns after `elapsed`: one a period.
pub fn cycle(elapsed: Duration, period: Duration) -> f64 {
    elapsed.as_secs_f64() / period.as_secs_f64().max(f64::EPSILON)
}

/// Whether a recipe has something that moves: a range (`rotate=-70..70`)
/// or a pattern (`rotate="0 40 80"`).
pub fn has_motion(words: &[String]) -> bool {
    words
        .iter()
        .filter_map(|word| word.split_once('='))
        .any(|(_, value)| {
            value.starts_with('"')
                || value.split(',').any(|part| {
                    // A minus sign is not the start of a range.
                    part.strip_prefix('-').unwrap_or(part).contains("..")
                })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: f64) -> f64 {
        moment(Duration::from_secs_f64(seconds), Duration::from_secs(2))
    }

    #[test]
    fn the_moment_goes_there_and_back_for_ever() {
        assert_eq!(at(0.0), 0.0);
        assert!((at(1.0) - 0.5).abs() < 1e-9, "halfway there");
        assert!((at(2.0) - 1.0).abs() < 1e-9, "the far end");
        assert!((at(3.0) - 0.5).abs() < 1e-9, "halfway back");
        assert!(at(4.0).abs() < 1e-9, "home");
        assert!((at(5.0) - at(1.0)).abs() < 1e-9, "again");
    }

    #[test]
    fn the_period_stays_within_bounds() {
        assert_eq!(scaled(Duration::from_secs(1), 0.001), SHORTEST);
        assert_eq!(scaled(Duration::from_secs(50), 100.0), LONGEST);
        assert_eq!(scaled(Duration::from_secs(2), 1.5), Duration::from_secs(3));
    }

    #[test]
    fn ranges_and_patterns_are_found_but_not_negative_numbers_or_dots() {
        let words = |text: &str| corrode_effects::split_words(text);
        assert!(has_motion(&words("seed=1 falsecolor rotate=-70..70")));
        assert!(has_motion(&words("seed=1 spin center=10..50,40")));
        assert!(has_motion(&words("seed=1 drag angle=-45..45")));
        assert!(has_motion(&words("seed=1 slice shift=\"10 20 30\"")));
        assert!(!has_motion(&words("seed=1 fade lift=-20 + grain size=2")));
        assert!(!has_motion(&words("seed=1 leak color=ff963c")));
        // An odd first character does not matter.
        assert!(!has_motion(&words("seed=1 leak color=é =")));
    }

    #[test]
    fn a_cycle_goes_by_each_period() {
        let period = Duration::from_secs(2);
        assert_eq!(cycle(Duration::from_secs(5), period), 2.5);
        assert_eq!(cycle(Duration::ZERO, period), 0.0);
    }
}
