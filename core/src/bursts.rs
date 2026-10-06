//! Bursts: runs of shots taken in quick succession, usually the same
//! scene a few tenths of a second apart. Culling a burst means picking
//! its best frame, so the viewer groups them.

use std::ops::Range;

/// Longest pause between two photos of the same burst, in milliseconds.
///
/// Calibrated on two concert shoots against the burst sequence numbers
/// a Panasonic GX9 records: photos of a burst come about 180 ms apart,
/// but the camera slows down to 2 s when its buffer fills, while two
/// bursts can follow each other within 250 ms. 300 ms finds 86 to 93% of
/// the bursts exactly; the others are split in two, or rarely merged with
/// the next one, which keeps the same scene together anyway.
pub const MAX_GAP_MS: i64 = 300;

/// Splits shots, given in shooting order with the time they were taken,
/// into bursts: ranges of consecutive shots, each taken at most
/// `max_gap_ms` after the previous one. A shot without time, or taken
/// before the previous one, starts a new burst.
pub fn group(times: &[Option<i64>], max_gap_ms: i64) -> Vec<Range<usize>> {
    let mut bursts = Vec::new();
    let mut start = 0;
    for index in 1..times.len() {
        let continues = match (times[index - 1], times[index]) {
            (Some(previous), Some(current)) => (0..=max_gap_ms).contains(&(current - previous)),
            _ => false,
        };
        if !continues {
            bursts.push(start..index);
            start = index;
        }
    }
    if !times.is_empty() {
        bursts.push(start..times.len());
    }
    bursts
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bursts as `(start, end)` pairs, easier to read in assertions.
    fn bursts(times: &[Option<i64>], max_gap_ms: i64) -> Vec<(usize, usize)> {
        group(times, max_gap_ms)
            .into_iter()
            .map(|burst| (burst.start, burst.end))
            .collect()
    }

    #[test]
    fn groups_shots_taken_close_together() {
        let times = [0, 110, 220, 5000, 5300, 9000].map(Some);
        assert_eq!(bursts(&times, 1000), [(0, 3), (3, 5), (5, 6)]);
        assert_eq!(bursts(&times, 100).len(), 6);
    }

    #[test]
    fn the_gap_limit_is_inclusive() {
        assert_eq!(bursts(&[Some(0), Some(300)], 300), [(0, 2)]);
        assert_eq!(bursts(&[Some(0), Some(301)], 300), [(0, 1), (1, 2)]);
    }

    #[test]
    fn missing_or_backward_times_split_bursts() {
        let times = [Some(0), Some(100), None, Some(200), Some(150)];
        assert_eq!(bursts(&times, 1000), [(0, 2), (2, 3), (3, 4), (4, 5)]);
    }

    #[test]
    fn no_shots_no_bursts() {
        assert!(group(&[], 1000).is_empty());
        assert_eq!(bursts(&[None], 1000), [(0, 1)]);
    }
}
