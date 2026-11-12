//! Scan-tab previews on a large scan, worked out beside the window. A Free Form's fit, Fit Patches'
//! patches and Flat Template's outline each look over the scan; on a scan of millions of triangles
//! that takes long enough to stop the window, so past [`LARGE`] triangles the work runs on a copy of
//! the session on a worker, the previous preview stays drawn while it works, and the dialog says
//! Refining… until the new one arrives. A result for values that have since changed is dropped. The
//! preview itself already works from a thinned sample of the scan (a few thousand to forty thousand
//! points), so what is worked out beside the window is the look over the whole scan for those
//! points. In the browser, or where no thread can start, the work is done here, as before.

use std::sync::mpsc::{Receiver, TryRecvError};

use solidcraft_engine::Session;

use crate::SolveApp;

/// The triangles from which a preview is worked out beside the window.
pub(crate) const LARGE: usize = 300_000;

thread_local! {
    /// For tests: treat every scan as large.
    static FORCED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Treat every scan as large (tests only).
#[cfg(test)]
pub(crate) fn force(on: bool) {
    FORCED.with(|f| f.set(on));
}

/// The most triangles any scan (mesh body) of the design has.
pub(crate) fn biggest_scan(app: &SolveApp) -> usize {
    app.session.model.state().bodies.iter().filter_map(|b| b.body.kept_mesh()).map(|m| m.triangles.len()).max().unwrap_or(0)
}

/// Whether previews are worked out beside the window now.
pub(crate) fn beside_window(app: &SolveApp) -> bool {
    cfg!(not(target_arch = "wasm32")) && (FORCED.with(std::cell::Cell::get) || biggest_scan(app) > LARGE)
}

/// A preview being worked out for the values `key`.
#[derive(Debug)]
pub(crate) struct Job<K, T> {
    pub key: K,
    done: std::sync::Mutex<Receiver<T>>,
}

/// What [`Job::poll`] found.
pub(crate) enum Poll<T> {
    Working,
    Done(T),
    /// The worker is gone without a result.
    Lost,
}

impl<K, T> Job<K, T> {
    pub(crate) fn poll(&self) -> Poll<T> {
        let Ok(done) = self.done.lock() else { return Poll::Lost };
        match done.try_recv() {
            Ok(t) => Poll::Done(t),
            Err(TryRecvError::Empty) => Poll::Working,
            Err(TryRecvError::Disconnected) => Poll::Lost,
        }
    }
}

/// Start `work` on a copy of the session beside the window for the values `key`. `None` where no
/// thread can start: the caller does the work itself.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn spawn<K, T: Send + 'static>(app: &SolveApp, key: K, work: impl FnOnce(&Session) -> T + Send + 'static) -> Option<Job<K, T>> {
    let copy = app.session.scratch();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("scan-preview".into())
        .spawn(move || {
            let _ = tx.send(work(&copy));
        })
        .ok()?;
    Some(Job { key, done: std::sync::Mutex::new(rx) })
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn spawn<K, T: Send + 'static>(_: &SolveApp, _: K, _: impl FnOnce(&Session) -> T + Send + 'static) -> Option<Job<K, T>> {
    None
}

/// The note a dialog shows while a preview is worked out, and the frames to come back for it.
pub(crate) fn refining_note(ui: &mut egui::Ui, id: &'static str) {
    crate::ui_kit::text(ui, crate::i18n::tr("ui.scan_refining"), crate::ui_kit::Text::Hint(id));
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
}
