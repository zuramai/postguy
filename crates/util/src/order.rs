//! Fractional ordering: compute a key strictly inside an open interval without renumbering
//! neighbours, for drag-reorder and insert operations. Keys are `u128` (wide enough for tens
//! of thousands of midpoints before a renumber).
//!
//! When an insert has no room (`None`), the caller renumbers that contiguous run — array
//! order remains the source of truth in this product; fractional keys are the
//! convenience, not the storage format.

/// The first key when there is nothing to be between: the centre, so both directions have the
/// same room before exhaustion.
pub const FIRST_KEY: u128 = 1 << 63;

/// A key strictly between `prev` and `next` (either may be absent = open end). `None` means
/// "no room: renumber this run".
///
/// Bookkeeping with [`order_allocations`]: keys used without care choose midpoint each time;
/// runs touching the floor (below the smallest key) are renumbered rather than key-hacked.
pub fn between(prev: Option<u128>, next: Option<u128>) -> Option<u128> {
    let (lo, hi) = match (prev, next) {
        (None, None) => return Some(FIRST_KEY),
        (None, Some(hi)) => (0, hi),
        (Some(lo), None) => return lo.checked_add(1),
        (Some(lo), Some(hi)) => (lo, hi),
    };
    if lo >= hi {
        return None;
    }
    let mid = lo + (hi - lo) / 2;
    if mid == lo { None } else { Some(mid) }
}

/// Renumber a run of `count` items evenly, starting at the centre.
pub fn renumber(count: usize) -> Vec<u128> {
    (0..count as u128).map(|i| FIRST_KEY.saturating_add(i)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midpoint_between_two_keys() {
        assert_eq!(between(Some(10), Some(30)), Some(20));
        assert_eq!(between(Some(10), Some(13)), Some(11));
    }

    #[test]
    fn ends_are_handled_sensibly() {
        assert_eq!(between(None, Some(10)), Some(5));
        assert_eq!(between(Some(10), None), Some(11));
    }

    #[test]
    fn adjacency_drops_to_renumber() {
        assert_eq!(between(Some(5), Some(6)), None);
        assert_eq!(between(Some(300), Some(298)), None, "inverted range is a renumber signal");
        assert_eq!(between(None, Some(0)), None, "nothing below key 0 without renumbering");
        let run = renumber(3);
        assert_eq!(between(Some(run[1]), Some(run[2])), None, "contiguous programmatically built run");
    }

    #[test]
    fn a_chain_of_mids_survives_many_inserts() {
        // 2^127 key space: hundreds of thousands of halvings before exhaustion; two rounds of
        // 40 halvings land nowhere near it.
        let mut lo = FIRST_KEY;
        let hi = lo + (1 << 100);
        let mut inserted = 0;
        for _ in 0..80 {
            let Some(mid) = between(Some(lo), Some(hi)) else { break };
            assert!(mid > lo && mid < hi);
            lo = mid;
            inserted += 1;
        }
        assert_eq!(inserted, 80);
    }

    #[test]
    fn renumber_is_stable_and_ordered() {
        let keys = renumber(5);
        assert_eq!(keys.len(), 5);
        for pair in keys.windows(2) {
            assert!(pair[0] < pair[1]);
        }
        assert_eq!(keys[0], FIRST_KEY);
    }
}
