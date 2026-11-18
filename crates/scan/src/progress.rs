//! How far long scan work is, for whoever waits on it (an agent's MCP client, the app's status):
//! the work reports its step with [`stage`], and the caller that wants to hear it runs the work
//! inside [`reporting`]. Work nobody listens to reports into nothing. Only the thread that runs
//! the work reports: the threads it shares the work out to don't.

use std::cell::RefCell;
use std::sync::Arc;

/// Told `done` of `total` units of step `stage` ("finding shapes").
pub type Hook = Arc<dyn Fn(&'static str, usize, usize) + Send + Sync>;

thread_local! {
    static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
}

/// Run `f` with `hook` told how far the scan work it does on this thread is.
pub fn reporting<R>(hook: Hook, f: impl FnOnce() -> R) -> R {
    let previous = HOOK.with(|h| h.replace(Some(hook)));
    struct Restore(Option<Option<Hook>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(previous) = self.0.take() {
                HOOK.with(|h| *h.borrow_mut() = previous);
            }
        }
    }
    let _restore = Restore(Some(previous));
    f()
}

/// Say that the work is `done` of `total` units into step `stage`.
pub fn stage(stage: &'static str, done: usize, total: usize) {
    HOOK.with(|h| {
        if let Some(hook) = h.borrow().as_ref() {
            hook(stage, done.min(total), total);
        }
    });
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[test]
    fn a_stage_reaches_the_hook_only_while_reporting() {
        let heard = Arc::new(Mutex::new(Vec::new()));
        let into = Arc::clone(&heard);
        stage("unheard", 1, 2);
        reporting(Arc::new(move |s, d, t| into.lock().unwrap().push((s, d, t))), || {
            stage("finding shapes", 3, 10);
            stage("finding shapes", 12, 10);
        });
        stage("after", 1, 1);
        assert_eq!(*heard.lock().unwrap(), [("finding shapes", 3, 10), ("finding shapes", 10, 10)]);
    }
}
