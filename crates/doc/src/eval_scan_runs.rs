//! One working scan for a run of Scan Clean steps. Each step's result is a new mesh of the whole
//! scan (tens of millions of triangles), and the evaluator keeps the state after every feature: a
//! run of steps would hold one copy of the scan per step. Instead, once a later step on the same
//! scan has read a step's result, the states holding that result give it up: they hold the scan
//! as it was before the run's first step (the import's, held anyway) and remember that they are
//! thinned. Only the scan before the run and the latest result stay.
//!
//! A thinned state is made whole again ([`materialized`]) where it is used: before a feature is
//! evaluated on it (a step edited, a feature added after a rolled-back marker), and when it is the
//! model's last state (the marker rolled back onto a step). Its steps are then done again, in
//! order, from the scan before the run, as the design is made again when it opens.

use super::*;
use crate::scan_clean::{self, CleanStep, Refused};
use std::sync::Weak;

/// A run of Scan Clean steps on one scan: the scan before its first step, each step with the
/// scans it fused in (Remesh's `with`), and the mesh the last step made.
#[derive(Debug)]
pub(crate) struct ScanRun {
    pub(crate) base: Body,
    pub(crate) steps: Vec<(CleanStep, Vec<Body>)>,
    pub(crate) latest: Weak<solidcraft_geom::Mesh>,
}

fn mesh_ptr(body: &Body) -> Option<*const solidcraft_geom::Mesh> {
    body.kept_mesh().map(|m| Arc::as_ptr(&m))
}

impl ScanRun {
    /// Whether `body` is this run's latest result.
    fn holds(&self, body: &Body) -> bool {
        match (self.latest.upgrade(), body.kept_mesh()) {
            (Some(a), Some(b)) => Arc::ptr_eq(&a, &b),
            _ => false,
        }
    }
}

/// The run `st`'s scan `name` is in after a step (`step`, fusing in `with`) read `input` and made
/// `output`: the run `input` ended, one step longer, or a new run from `input`.
pub(crate) fn note_step(st: &mut ModelState, name: &str, input: &Body, output: &Body, step: &CleanStep, with: &[&Body]) {
    let run = st.scan_runs.get(name).filter(|r| r.holds(input));
    let (base, mut steps) = match run {
        Some(r) => (r.base.clone(), r.steps.clone()),
        None => (input.clone(), Vec::new()),
    };
    steps.push((step.clone(), with.iter().map(|b| (*b).clone()).collect()));
    let latest = output.kept_mesh().map(|m| Arc::downgrade(&m)).unwrap_or_default();
    st.scan_runs.insert(name.to_owned(), Arc::new(ScanRun { base, steps, latest }));
}

/// `st` with each thinned scan made again from the scan before its run.
pub(crate) fn materialized(st: &ModelState) -> ModelState {
    let mut out = st.clone();
    for name in std::mem::take(&mut out.thinned) {
        let Some(run) = out.scan_runs.get(&name).cloned() else { continue };
        let Some(index) = out.bodies.iter().position(|b| b.name == name) else { continue };
        let mut body = run.base.clone();
        for (step, with) in &run.steps {
            let with: Vec<&Body> = with.iter().collect();
            match scan_clean::clean(step, &body, &with) {
                Ok(done) => body = done.body,
                Err(Refused::Nothing(_)) => {}
                // It was done before on this same scan: it cannot fail now. Should it, the scan is
                // left as far as it got rather than as it was before the run.
                Err(Refused::Failed(why)) => {
                    log::warn!("a Scan Clean step on `{name}` made again failed: {why}");
                    break;
                }
            }
        }
        let shell = &out.bodies[index];
        let color = shell.body.color();
        out.bodies[index] = shell.with_geometry(body.with_color(color));
        // The run's latest result is this one again.
        if let Some(m) = out.bodies[index].body.kept_mesh() {
            out.scan_runs.insert(name, Arc::new(ScanRun { base: run.base.clone(), steps: run.steps.clone(), latest: Arc::downgrade(&m) }));
        }
    }
    out
}

/// The Scan Clean steps of `doc`, by feature id: the scan each works on.
pub(crate) fn steps_of(doc: &Document) -> std::collections::HashMap<u64, String> {
    doc.features
        .iter()
        .filter_map(|f| match &f.kind {
            FeatureKind::ScanClean { body, .. } => Some((f.id, body.0.clone())),
            _ => None,
        })
        .collect()
}

/// Thins the states in `results` whose scan a later Scan Clean step (`steps`, by feature id) has
/// made again: each gives up its copy of the scan and holds the scan before its run. A state
/// several results share is thinned once; the last state is never thinned.
pub(crate) fn thin(results: &mut [FeatureResult], steps: &std::collections::HashMap<u64, String>) {
    // For each scan, the meshes later steps made of it.
    let mut later: BTreeMap<String, Vec<*const solidcraft_geom::Mesh>> = BTreeMap::new();
    let mut done: std::collections::HashMap<*const ModelState, Arc<ModelState>> = std::collections::HashMap::new();
    let last = results.iter().rposition(|r| !r.skipped).map(|i| Arc::as_ptr(&results[i].state));
    for r in results.iter_mut().rev() {
        if r.skipped {
            continue;
        }
        let key = Arc::as_ptr(&r.state);
        if let Some(thinned) = done.get(&key) {
            r.state = thinned.clone();
        } else if Some(key) != last {
            let mut give_up = Vec::new();
            for (name, run) in &r.state.scan_runs {
                let Some(b) = r.state.body(name).filter(|b| !r.state.thinned.contains(name) && run.holds(&b.body)) else { continue };
                let mine = mesh_ptr(&b.body);
                if later.get(name).is_some_and(|made| made.iter().any(|m| Some(*m) != mine)) {
                    give_up.push((name.clone(), run.base.clone()));
                }
            }
            if !give_up.is_empty() {
                let mut st = (*r.state).clone();
                for (name, base) in give_up {
                    if let Some(b) = st.bodies.iter_mut().find(|b| b.name == name) {
                        let color = b.body.color();
                        *b = b.with_geometry(base.with_color(color));
                        st.thinned.insert(name);
                    }
                }
                let st = Arc::new(st);
                done.insert(key, st.clone());
                r.state = st;
            }
        }
        // A step that made the scan again (not one that failed or found nothing to do).
        if r.error.is_none()
            && let Some(name) = steps.get(&r.id)
            && let Some(m) = r.state.body(name).and_then(|b| mesh_ptr(&b.body))
        {
            later.entry(name.clone()).or_default().push(m);
        }
    }
}

/// Makes the last state of `results` whole (and the skipped results after it, which share it):
/// the model's state is never a thinned one.
pub(crate) fn whole_last(results: &mut [FeatureResult]) {
    let Some(last) = results.iter().rposition(|r| !r.skipped) else { return };
    if results[last].state.thinned.is_empty() {
        return;
    }
    let key = Arc::as_ptr(&results[last].state);
    let whole = Arc::new(materialized(&results[last].state));
    for r in results[last..].iter_mut().filter(|r| Arc::as_ptr(&r.state) == key) {
        r.state = whole.clone();
    }
}

/// What a design file left out after a Scan Clean step stands in as, when the design opens: the
/// state before the step, its scan thinned and the step added to its run, so the step is done
/// only if something reads that state ([`materialized`]), not to be thinned again at once. `None`
/// unless `f` is a Scan Clean step whose scan, and the scans it fuses in, `before` holds whole
/// or as a run's thinned scan.
pub(crate) fn stand_in(before: &ModelState, f: &Feature) -> Option<ModelState> {
    let FeatureKind::ScanClean { body, step } = &f.kind else { return None };
    // A step that names parameters is run from the values of the design: none stand in for it.
    if step.needs_values() {
        return None;
    }
    let name = body.0.as_str();
    let scan = before.body(name)?;
    let mut with = Vec::new();
    for w in step.with() {
        // A scan fused in must be whole: a thinned one is not the scan the step read.
        if before.thinned.contains(&w.0) {
            return None;
        }
        with.push(before.body(&w.0)?.body.clone());
    }
    let (base, mut steps) = match before.scan_runs.get(name).filter(|_| before.thinned.contains(name)) {
        Some(run) => (run.base.clone(), run.steps.clone()),
        None => (scan.body.clone(), Vec::new()),
    };
    steps.push((step.clone(), with));
    let mut st = before.clone();
    st.scan_runs.insert(name.to_owned(), Arc::new(ScanRun { base, steps, latest: Weak::new() }));
    st.thinned.insert(name.to_owned());
    Some(st)
}
