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
