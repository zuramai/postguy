//! Bounded FIFO ring buffer for the log-like panels (console, timelines): fixed cap, oldest
//! dropped first, and the dropped item is returned so callers can keep running counters.

/// A FIFO of at most `cap` items. `push` evicts from the front when the cap is reached and
/// hands the evicted item back, so the caller can track what it dropped without iterating.
pub struct Ring<T> {
    items: std::collections::VecDeque<T>,
    cap: usize,
    dropped_total: u64,
}

impl<T> Ring<T> {
    /// A ring holding at most `cap` items. `cap` of 0 is legal but useless: it drops
    /// everything pushed.
    pub fn new(cap: usize) -> Self {
        Self { items: std::collections::VecDeque::with_capacity(cap.min(1024)), cap, dropped_total: 0 }
    }

    /// Push at the back; when full, the front item is evicted and returned.
    pub fn push(&mut self, item: T) -> Option<T> {
        if self.cap == 0 {
            self.dropped_total += 1;
            return Some(item);
        }
        let evicted = if self.items.len() >= self.cap { self.items.pop_front() } else { None };
        if evicted.is_some() {
            self.dropped_total += 1;
        }
        self.items.push_back(item);
        evicted
    }

    /// Are there items to iterate?
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Live item count (excluding dropped ones).
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// How many items have been dropped by the cap, over the ring's lifetime.
    pub fn dropped_total(&self) -> u64 {
        self.dropped_total
    }

    /// The cap this ring was built with.
    pub fn cap(&self) -> usize {
        self.cap
    }

    /// Front-to-back iteration.
    pub fn iter(&self) -> std::collections::vec_deque::Iter<'_, T> {
        self.items.iter()
    }

    /// Back-to-front iteration (newest first for reverse-chronological views).
    pub fn iter_rev(&self) -> std::iter::Rev<std::collections::vec_deque::Iter<'_, T>> {
        self.items.iter().rev()
    }

    /// Drop everything; the count of items removed is returned.
    pub fn clear(&mut self) -> usize {
        let removed = self.items.len();
        self.items.clear();
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_from_the_front_when_cap_reached() {
        let mut ring = Ring::new(3);
        assert_eq!(ring.push(1), None);
        assert_eq!(ring.push(2), None);
        assert_eq!(ring.push(3), None);
        assert_eq!(ring.push(4), Some(1));
        let items: Vec<i32> = ring.iter().copied().collect();
        assert_eq!(items, vec![2, 3, 4]);
        assert_eq!(ring.dropped_total(), 1);
        assert_eq!(ring.len(), 3);
    }

    #[test]
    fn zero_cap_drops_everything() {
        let mut ring = Ring::<u8>::new(0);
        assert_eq!(ring.push(7), Some(7));
        assert!(ring.is_empty());
        assert_eq!(ring.dropped_total(), 1);
    }

    #[test]
    fn reverse_iteration_is_newest_first() {
        let mut ring = Ring::new(4);
        for n in 1..=4 {
            ring.push(n);
        }
        let rev: Vec<i32> = ring.iter_rev().copied().collect();
        assert_eq!(rev, vec![4, 3, 2, 1]);
    }

    #[test]
    fn clear_reports_how_much_it_removed() {
        let mut ring = Ring::new(8);
        for n in 0..3 {
            ring.push(n);
        }
        assert_eq!(ring.clear(), 3);
        assert!(ring.is_empty());
        assert_eq!(ring.clear(), 0);
    }
}
