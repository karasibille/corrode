//! A small deterministic random source, enough for the effects to draw
//! positions and sizes, so that a seed names a result on any machine.

/// SplitMix64.
pub(super) struct Random(u64);

impl Random {
    pub(super) fn new(seed: u64) -> Random {
        Random(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `0..bound`; 0 when the bound is 0.
    pub(super) fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 { 0 } else { self.next() % bound }
    }

    /// A number in `low..=high`.
    pub(super) fn between(&mut self, low: u32, high: u32) -> u32 {
        let (low, high) = (low.min(high), low.max(high));
        low + self.below(u64::from(high - low) + 1) as u32
    }

    /// A number in `-extent..=extent`.
    pub(super) fn around(&mut self, extent: u32) -> i32 {
        self.below(2 * u64::from(extent) + 1) as i32 - extent as i32
    }

    /// True once in `n` times.
    pub(super) fn once_in(&mut self, n: u64) -> bool {
        self.below(n) == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_stay_in_range_and_follow_the_seed() {
        let mut random = Random::new(3);
        for _ in 0..1000 {
            assert!(random.below(7) < 7);
            let n = random.between(10, 20);
            assert!((10..=20).contains(&n));
            let d = random.around(5);
            assert!((-5..=5).contains(&d));
        }
        assert_eq!(random.below(0), 0);
        assert_eq!(random.between(4, 4), 4);
        assert_eq!(random.around(0), 0);

        let draws = |seed| {
            let mut random = Random::new(seed);
            (0..5).map(|_| random.below(1000)).collect::<Vec<_>>()
        };
        assert_eq!(draws(1), draws(1));
        assert_ne!(draws(1), draws(2));
    }
}
