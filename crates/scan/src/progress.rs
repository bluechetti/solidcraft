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

    #[test]
    fn reporting_returns_what_the_work_returns() {
        let heard = Arc::new(Mutex::new(Vec::new()));
        assert_eq!(reporting(Arc::new(move |_s, _d, _t| heard.lock().unwrap().push(("", 0, 0))), || 7), 7);
    }

    #[test]
    fn a_total_of_zero_reports_zero_of_zero() {
        let heard = Arc::new(Mutex::new(Vec::new()));
        let into = Arc::clone(&heard);
        reporting(Arc::new(move |s, d, t| into.lock().unwrap().push((s, d, t))), || {
            stage("empty", 5, 0);
        });
        assert_eq!(*heard.lock().unwrap(), [("empty", 0, 0)]);
    }

    #[test]
    fn a_nested_reporting_goes_to_the_inner_hook_and_then_back_to_the_outer() {
        let outer_heard = Arc::new(Mutex::new(Vec::new()));
        let outer_into = Arc::clone(&outer_heard);
        let inner_heard = Arc::new(Mutex::new(Vec::new()));
        let inner_into = Arc::clone(&inner_heard);
        reporting(Arc::new(move |s, d, t| outer_into.lock().unwrap().push((s, d, t))), || {
            stage("a", 1, 2);
            reporting(Arc::new(move |s, d, t| inner_into.lock().unwrap().push((s, d, t))), || {
                stage("b", 1, 2);
            });
            stage("c", 2, 2);
        });
        assert_eq!(*outer_heard.lock().unwrap(), [("a", 1, 2), ("c", 2, 2)]);
        assert_eq!(*inner_heard.lock().unwrap(), [("b", 1, 2)]);
    }

    #[test]
    fn the_previous_hook_comes_back_when_the_work_panics() {
        let outer_heard = Arc::new(Mutex::new(Vec::new()));
        let outer_into = Arc::clone(&outer_heard);
        let inner_heard = Arc::new(Mutex::new(Vec::new()));
        let inner_into = Arc::clone(&inner_heard);
        let inner: Hook = Arc::new(move |_s, _d, _t| inner_into.lock().unwrap().push(("", 0, 0)));
        reporting(Arc::new(move |s, d, t| outer_into.lock().unwrap().push((s, d, t))), || {
            let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                reporting(Arc::clone(&inner), || panic!("work failed"));
            }));
            assert!(panicked.is_err());
            stage("after", 1, 1);
        });
        assert_eq!(*outer_heard.lock().unwrap(), [("after", 1, 1)]);
        assert!(inner_heard.lock().unwrap().is_empty());
    }

    #[test]
    fn work_shared_out_to_other_threads_reports_nothing() {
        let heard = Arc::new(Mutex::new(Vec::new()));
        let into = Arc::clone(&heard);
        reporting(Arc::new(move |_s, _d, _t| into.lock().unwrap().push(("", 0, 0))), || {
            assert!(std::thread::spawn(|| stage("elsewhere", 1, 1)).join().is_ok());
        });
        assert!(heard.lock().unwrap().is_empty());
    }
}
