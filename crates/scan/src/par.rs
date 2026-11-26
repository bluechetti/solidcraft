//! Ordered parallel maps over independent items: on the desktop over rayon's pool, in the browser
//! build on the calling thread. The results are in item order whatever the threads, so the same
//! scan gives the same answer.

/// `f` of each of `0..count`, in order.
pub(crate) fn map<R: Send>(count: usize, f: impl Fn(usize) -> R + Sync + Send) -> Vec<R> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use rayon::prelude::*;
        (0..count).into_par_iter().map(f).collect()
    }
    #[cfg(target_arch = "wasm32")]
    {
        (0..count).map(f).collect()
    }
}

/// `items` sorted, on rayon's pool on the desktop (a scan's tens of millions of sides take
/// seconds on one thread), in the browser build on the calling thread. The same order either way.
pub(crate) fn sort_unstable<T: Ord + Send>(items: &mut [T]) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use rayon::prelude::*;
        items.par_sort_unstable();
    }
    #[cfg(target_arch = "wasm32")]
    {
        items.sort_unstable();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_returns_results_in_item_order() {
        let got = map(1000, |i| i * 3);
        let want: Vec<_> = (0..1000).map(|i| i * 3).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn map_of_nothing_is_empty() {
        let got: Vec<usize> = map(0, |i| i);
        assert!(got.is_empty());
    }

    #[test]
    fn map_runs_each_item_exactly_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = AtomicUsize::new(0);
        let got = map(500, |i| {
            calls.fetch_add(1, Ordering::Relaxed);
            i
        });
        let want: Vec<_> = (0..500).collect();
        assert_eq!(got, want);
        assert_eq!(calls.load(Ordering::Relaxed), 500);
    }

    #[test]
    fn sort_unstable_matches_the_standard_sort() {
        let mut items = Vec::with_capacity(5000);
        let mut x: u64 = 1;
        for _ in 0..5000 {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            items.push(x >> 11);
        }
        let mut par = items.clone();
        let mut reference = items.clone();
        sort_unstable(&mut par);
        reference.sort();
        assert_eq!(par, reference);
    }

    #[test]
    fn sort_unstable_leaves_nothing_and_one_item_alone() {
        let mut empty: Vec<u64> = Vec::new();
        sort_unstable(&mut empty);
        assert!(empty.is_empty());
        let mut one = vec![7];
        sort_unstable(&mut one);
        assert_eq!(one, vec![7]);
    }
}
