//! Scan Triangles (`view.scan_triangles`): the triangles of the scans shown, drawn as lines over
//! them, to see how fine a scan is and what Reduce Mesh or Remesh made of it. Display only. A scan
//! of more than [`BUDGET`] triangles shows those of a copy clustered down to about that many
//! ([`solidcraft_engine::geom::simplify::clustered`]): its lines would be more than the screen has
//! pixels. A scan's lines are made once, beside the window when it is large.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use solidcore_record::Vec3;
use solidcraft_engine::geom::Mesh;
use solidcraft_engine::geom::prelude::*;

use crate::SolveApp;

/// The most triangles a scan's lines are drawn from.
pub(crate) const BUDGET: usize = 500_000;
/// A scan this small has its lines made on the spot.
const INLINE: usize = 50_000;
/// Scans whose lines are made at once.
const MAX_JOBS: usize = 2;

/// A scan's triangle edges, each once.
pub(crate) type Lines = Arc<Vec<[[f32; 3]; 2]>>;

#[derive(Default)]
struct Store {
    /// By mesh address, the mesh held so its address stays its own; `None` while being made.
    by_mesh: HashMap<usize, (Arc<Mesh>, Option<Lines>)>,
    /// Counts the scans whose lines were made beside the window: the scene is built again then.
    generation: u64,
    making: usize,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);

fn with_store<R>(f: impl FnOnce(&mut Store) -> R) -> Option<R> {
    let mut held = STORE.lock().ok()?;
    Some(f(held.get_or_insert_with(Store::default)))
}

/// What the scene's lines depend on beyond the model: the setting, and lines made since.
pub(crate) fn scene_key(app: &SolveApp) -> u64 {
    if app.ui.scan_triangles { 1 + 2 * with_store(|s| s.generation).unwrap_or(0) } else { 0 }
}

/// Lines are being made (keep frames coming to draw them).
pub(crate) fn busy() -> bool {
    with_store(|s| s.making > 0).unwrap_or(false)
}

/// `mesh`'s lines when made; otherwise `None`, and they are started (once).
pub(crate) fn lines(mesh: &Arc<Mesh>) -> Option<Lines> {
    let key = Arc::as_ptr(mesh) as usize;
    let start = with_store(|s| match s.by_mesh.get(&key).filter(|(held, _)| Arc::ptr_eq(held, mesh)) {
        Some((_, made)) => Err(made.clone()),
        None if mesh.triangles.len() <= INLINE => Ok(true),
        None if s.making >= MAX_JOBS => Err(None),
        None => {
            s.by_mesh.insert(key, (mesh.clone(), None));
            s.making += 1;
            Ok(false)
        }
    })?;
    match start {
        Err(found) => found,
        Ok(true) => {
            let made: Lines = Arc::new(made_lines(mesh));
            with_store(|s| s.by_mesh.insert(key, (mesh.clone(), Some(made.clone()))));
            Some(made)
        }
        Ok(false) => {
            let source = mesh.clone();
            let job = move || {
                let made: Lines = Arc::new(made_lines(&source));
                with_store(|s| {
                    if let Some(slot) = s.by_mesh.get_mut(&key).filter(|(held, _)| Arc::ptr_eq(held, &source)) {
                        slot.1 = Some(made);
                    }
                    s.making = s.making.saturating_sub(1);
                    s.generation = s.generation.wrapping_add(1);
                });
            };
            if std::thread::Builder::new().name("scan-triangles".into()).spawn(job).is_err() {
                // No threads (the browser): a large scan's lines are not drawn.
                with_store(|s| {
                    s.by_mesh.insert(key, (mesh.clone(), Some(Arc::new(Vec::new()))));
                    s.making = s.making.saturating_sub(1);
                });
            }
            None
        }
    }
}

/// Let go of the lines of meshes no longer shown (`kept`: the addresses of those that are).
pub(crate) fn retain(kept: &[usize]) {
    with_store(|s| s.by_mesh.retain(|k, (_, made)| kept.contains(k) || made.is_none()));
}

/// Each edge of the triangles once (points at one place counted as one), from a clustered copy
/// when there are more than [`BUDGET`] triangles.
fn made_lines(mesh: &Mesh) -> Vec<[[f32; 3]; 2]> {
    let clustered;
    let (positions, triangles): (&[Vec3], &[[u32; 3]]) = if mesh.triangles.len() > BUDGET {
        let area: f64 = mesh
            .triangles
            .iter()
            .filter_map(|t| {
                let [a, b, c] = t.map(|i| mesh.positions.get(i as usize).copied());
                Some((b? - a?).cross(c? - a?).len() / 2.0)
            })
            .filter(|a| a.is_finite())
            .sum();
        // Two triangles to a cube's face on the surface.
        let cell = (2.0 * area / BUDGET as f64).sqrt();
        clustered = solidcraft_engine::geom::simplify::clustered(&mesh.positions, &mesh.triangles, cell);
        (&clustered.0, &clustered.1)
    } else {
        (&mesh.positions, &mesh.triangles)
    };
    let mut ids: HashMap<[u32; 3], u32> = HashMap::with_capacity(positions.len());
    let id: Vec<u32> = positions
        .iter()
        .map(|p| {
            let next = ids.len() as u32;
            *ids.entry([p.x as f32, p.y as f32, p.z as f32].map(|v| (v + 0.0).to_bits())).or_insert(next)
        })
        .collect();
    let mut keys: Vec<(u32, u32, u32, u32)> = Vec::with_capacity(triangles.len() * 3);
    for t in triangles {
        let Some(ends) = t.iter().map(|&i| id.get(i as usize).map(|&k| (k, i))).collect::<Option<Vec<_>>>() else { continue };
        for k in 0..3 {
            let ((a, ia), (b, ib)) = (ends[k], ends[(k + 1) % 3]);
            if a != b {
                keys.push(if a < b { (a, b, ia, ib) } else { (b, a, ib, ia) });
            }
        }
    }
    keys.sort_unstable();
    keys.dedup_by_key(|k| (k.0, k.1));
    let at = |i: u32| positions.get(i as usize).map(|p| [p.x as f32, p.y as f32, p.z as f32]);
    keys.iter().filter_map(|&(_, _, i, j)| Some([at(i)?, at(j)?])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A square of two triangles has five edges, the diagonal once; a scan past the budget is
    /// drawn from about the budget's lines, not its own.
    #[test]
    fn each_edge_is_drawn_once_and_a_large_scan_from_fewer() {
        let square = Mesh {
            positions: vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 0.0), Vec3::new(0.0, 1.0, 0.0)],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            ..Mesh::default()
        };
        assert_eq!(made_lines(&square).len(), 5);
        let n = 600u32;
        let mut big = Mesh::default();
        for y in 0..=n {
            for x in 0..=n {
                big.positions.push(Vec3::new(f64::from(x), f64::from(y), 0.0));
            }
        }
        let v = |x: u32, y: u32| y * (n + 1) + x;
        for y in 0..n {
            for x in 0..n {
                big.triangles.push([v(x, y), v(x + 1, y), v(x + 1, y + 1)]);
                big.triangles.push([v(x, y), v(x + 1, y + 1), v(x, y + 1)]);
            }
        }
        assert!(big.triangles.len() > BUDGET);
        let drawn = made_lines(&big).len();
        assert!(drawn < 3 * BUDGET / 2 + 10_000 && drawn > BUDGET / 4, "{drawn} lines");
    }
}
