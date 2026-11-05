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
