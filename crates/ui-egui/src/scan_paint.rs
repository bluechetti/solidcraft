//! Colours painted on a scan's own triangles in the viewport: Scan Deviation's map, and the
//! colouring of Flat Template, Free Form, Fit Patches and Edit Surface. Each vertex of the scan
//! gets a colour (4 bytes on the GPU) that the scan's instanced triangles are drawn in
//! ([`solidcraft_viewport_gpu::view::InstancePaint`]), so a scan of a hundred million triangles is
//! painted whole, at every zoom, depth-tested with the model as the scan itself is. A painted
//! cell for each triangle (132 bytes) was the way before; past a million triangles (a quarter of a
//! million for the surface tools) it fell back to dots. Wireframe still draws those.
//!
//! A scan drawn from a coarser level of detail (small on screen) is painted on that level's own
//! vertices: each takes the strongest colour of the scan's vertices nearest it, so a spike far off
//! the model still shows from afar rather than being averaged away. The levels are painted beside
//! the window; until a level is, the scan is drawn from the finer ones that are.
//!
//! Only shaded views draw the scan's triangles this way; in wireframe the tools draw their
//! colouring as before ([`on_scan`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use solidcore_record::Vec3;
use solidcraft_engine::geom::Mesh;
use solidcraft_viewport_gpu::view::{InstancePaint, SharedTris, paint_colour};

use crate::SolveApp;
use crate::scan_deviation::Colours;

/// A scan painted: which, an identity of the colouring (a new one whenever it changes), where
/// each of its vertices sits on the colour scale (-1 to 1, 0 within the tolerance; not a number
/// for a vertex left unpainted), and the scale's colours.
#[derive(Clone)]
pub(crate) struct Painting {
    pub scan: String,
    pub key: u64,
    pub scale: Arc<Vec<f32>>,
    pub colours: Colours,
}

/// Whether the scans' triangles are painted in the view now (a shaded view on the GPU): when not,
/// the tools draw their colouring over the scan as before.
pub(crate) fn on_scan(app: &SolveApp) -> bool {
    app.ui.visual_style != 2 && app.viewport.gpu.is_some()
}

/// Every scan painted now.
pub(crate) fn paintings(app: &SolveApp) -> Vec<Painting> {
    if !on_scan(app) {
        return Vec::new();
    }
    crate::scan_deviation::painting(app)
        .into_iter()
        .chain(crate::dialogs_scan_template::painting(app))
        .chain(crate::dialogs_scan_free_form::painting(app))
        .chain(crate::dialogs_scan_patches::painting(app))
        .chain(crate::dialogs_scan_surface::edit_painting(app))
        .collect()
}

/// What the model scene's key takes from the painting: the scans painted, their colourings and
/// how many of their levels are painted yet.
pub(crate) fn scene_key(app: &SolveApp) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in paintings(app) {
        p.scan.hash(&mut h);
        p.key.hash(&mut h);
    }
    generation().hash(&mut h);
    h.finish()
}

/// A painting's colour for a place on the scale.
fn colour_of(colours: Colours, s: f32) -> u32 {
    if s.is_nan() { 0 } else { paint_colour(crate::scan_deviation::colour(colours, f64::from(s))) }
}

/// The colours for `painting` on the triangles `tris` made from `mesh` (the scan's), and on each of
/// `levels`, as far as they are painted yet: the paint, and the levels to draw (the finest ones
/// painted, in order). `None` when the scan's colours do not fit its triangles (it changed).
pub(crate) fn paint(
    painting: &Painting,
    mesh: &Arc<Mesh>,
    tris: &Arc<SharedTris>,
    levels: &[(Arc<SharedTris>, f32)],
) -> Option<(Arc<InstancePaint>, Vec<(Arc<SharedTris>, f32)>)> {
    if painting.scale.len() != mesh.positions.len() {
        return None;
    }
    let whole = whole_colours(painting, mesh, tris)?;
    let mut painted = vec![whole];
    let mut drawn = Vec::new();
    for (level, error) in levels {
        let Some(colours) = level_colours(painting, mesh, level) else { break };
        painted.push(colours);
        drawn.push((level.clone(), *error));
    }
    Some((Arc::new(InstancePaint { levels: painted }), drawn))
}

/// What has been worked out, kept while it is used: the colours of a scan's own vertices, by the
/// painting and the triangles; and each level's, by the painting and the level.
#[derive(Default)]
struct Store {
    whole: HashMap<(u64, usize), (Arc<SharedTris>, Arc<Vec<u32>>)>,
    levels: HashMap<(u64, usize), Level>,
    /// Counts the levels painted: the scene is built again when it changes.
    generation: u64,
    /// Levels being painted beside the window.
    making: usize,
}

enum Level {
    Making(Arc<SharedTris>),
    Made(Arc<SharedTris>, Option<Arc<Vec<u32>>>),
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);

fn with_store<R>(f: impl FnOnce(&mut Store) -> R) -> Option<R> {
    let mut held = STORE.lock().ok()?;
    Some(f(held.get_or_insert_with(Store::default)))
}

/// Changes whenever a level is painted.
fn generation() -> u64 {
    with_store(|s| s.generation).unwrap_or(0)
}

/// Levels are being painted (keep frames coming to pick them up).
pub(crate) fn busy() -> bool {
    with_store(|s| s.making > 0).unwrap_or(false)
}

/// Let go of what no painting now uses: called once a scene is built.
pub(crate) fn retain(app: &SolveApp) {
    let keys: Vec<u64> = paintings(app).iter().map(|p| p.key).collect();
    with_store(|s| {
        s.whole.retain(|(k, _), _| keys.contains(k));
        s.levels.retain(|(k, _), l| keys.contains(k) || matches!(l, Level::Making(_)));
    });
}

/// The colours of the scan's own vertices for `tris` (its triangles as drawn): one for each of the
/// mesh's vertices when they are the mesh's, else carried over to the triangles' own.
fn whole_colours(painting: &Painting, mesh: &Mesh, tris: &Arc<SharedTris>) -> Option<Arc<Vec<u32>>> {
    let at = (painting.key, Arc::as_ptr(tris) as usize);
    if let Some(found) = with_store(|s| s.whole.get(&at).filter(|(t, _)| Arc::ptr_eq(t, tris)).map(|(_, c)| c.clone())).flatten() {
        return Some(found);
    }
    let per_vertex: Vec<u32> = painting.scale.iter().map(|s| colour_of(painting.colours, *s)).collect();
    let made = if tris.shares_mesh_vertices() { per_vertex } else { tris.from_mesh_vertices(&mesh.triangles, &per_vertex)? };
    let made = Arc::new(made);
    with_store(|s| s.whole.insert(at, (tris.clone(), made.clone())));
    Some(made)
}

/// `level`'s colours when they are painted; started beside the window (once) when not. `None`
/// until then, or when the level could not be painted.
fn level_colours(painting: &Painting, mesh: &Arc<Mesh>, level: &Arc<SharedTris>) -> Option<Arc<Vec<u32>>> {
    let at = (painting.key, Arc::as_ptr(level) as usize);
    let start = with_store(|s| match s.levels.get(&at) {
        Some(Level::Made(held, colours)) if Arc::ptr_eq(held, level) => Err(colours.clone()),
        Some(Level::Making(held)) if Arc::ptr_eq(held, level) => Err(None),
        _ if s.making >= MAX_JOBS => Err(None),
        _ => {
            s.levels.insert(at, Level::Making(level.clone()));
            s.making += 1;
            Ok(())
        }
    })?;
    if let Err(found) = start {
        return found;
    }
    let (scale, colours, mesh, made) = (painting.scale.clone(), painting.colours, mesh.clone(), level.clone());
    let job = move || {
        let painted = strongest_on(&mesh.positions, &scale, &made);
        let painted = painted.map(|scale| Arc::new(scale.iter().map(|s| colour_of(colours, *s)).collect::<Vec<u32>>()));
        with_store(|s| {
            s.levels.insert(at, Level::Made(made, painted));
            s.making = s.making.saturating_sub(1);
            s.generation = s.generation.wrapping_add(1);
        });
    };
    if std::thread::Builder::new().name("scan-paint".into()).spawn(job).is_err() {
        // No threads (the browser): the level is not drawn while the scan is painted.
        with_store(|s| {
            s.levels.insert(at, Level::Made(level.clone(), None));
            s.making = s.making.saturating_sub(1);
        });
    }
    None
}

/// Levels painted at once.
const MAX_JOBS: usize = 2;

/// Each vertex of `level` on the scale: the strongest place (farthest from 0, its sign kept) of
/// the scan's vertices nearest it (`positions`, on `scale`), so a spike is not averaged away on a
/// coarse level. A level vertex no scan vertex is nearest takes its neighbours' strongest. `None`
/// for a level with no vertices or not in a box.
fn strongest_on(positions: &[Vec3], scale: &[f32], level: &SharedTris) -> Option<Vec<f32>> {
    let at: Vec<[f32; 3]> = level.positions().collect();
    let grid = Grid::over(&at, level.indices().as_chunks::<3>().0)?;
    let mut out = vec![f32::NAN; at.len()];
    for (p, s) in positions.iter().zip(scale) {
        if s.is_nan() {
            continue;
        }
        let Some(v) = grid.nearest(&at, [p.x as f32, p.y as f32, p.z as f32]) else { continue };
        let slot = &mut out[v];
        if slot.is_nan() || s.abs() > slot.abs() {
            *slot = *s;
        }
    }
    // A level vertex no scan vertex came to (it lies between them): its neighbours' strongest,
    // twice over, so no unpainted speck shows in a painted area.
    let triangles = level.indices().as_chunks::<3>().0;
    for _ in 0..2 {
        let before = out.clone();
        for t in triangles {
            for (k, i) in t.iter().enumerate() {
                let Some(here) = before.get(*i as usize) else { continue };
                if !here.is_nan() {
                    continue;
                }
                let others = [t[(k + 1) % 3], t[(k + 2) % 3]];
                let near = others.iter().filter_map(|j| before.get(*j as usize).copied()).filter(|x| !x.is_nan());
                if let Some(best) = near.max_by(|a, b| a.abs().total_cmp(&b.abs()))
                    && let Some(slot) = out.get_mut(*i as usize)
                    && (slot.is_nan() || best.abs() > slot.abs())
                {
                    *slot = best;
                }
            }
        }
    }
    Some(out)
}

/// A level's vertices in cubes, to find the one nearest a place.
struct Grid {
    lo: [f32; 3],
    cell: f32,
    cubes: HashMap<[i32; 3], Vec<u32>>,
}

/// Triangles whose sides the cube side is read from, at most.
const SIDES_READ: usize = 20_000;

impl Grid {
    /// `at` (a level's vertices) in cubes twice as wide as the middle side of its `triangles`: not
    /// read from the box round them, which a speck floating far off the scan would make so large
    /// that every vertex fell in one cube.
    fn over(at: &[[f32; 3]], triangles: &[[u32; 3]]) -> Option<Grid> {
        let mut lo = [f32::INFINITY; 3];
        for p in at.iter().filter(|p| p.iter().all(|x| x.is_finite())) {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
            }
        }
        let step = (triangles.len() / SIDES_READ).max(1);
        let mut sides: Vec<f32> = triangles
            .iter()
            .step_by(step)
            .filter_map(|t| {
                let (a, b) = (at.get(t[0] as usize)?, at.get(t[1] as usize)?);
                Some((0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>().sqrt())
            })
            .filter(|d| d.is_finite() && *d > 0.0)
            .collect();
        if sides.is_empty() || !lo.iter().all(|x| x.is_finite()) {
            return None;
        }
        let middle = sides.len() / 2;
        let cell = 2.0 * *sides.select_nth_unstable_by(middle, f32::total_cmp).1;
        let mut grid = Grid { lo, cell, cubes: HashMap::new() };
        for (i, p) in at.iter().enumerate() {
            if let Some(c) = grid.cube(*p) {
                grid.cubes.entry(c).or_default().push(i as u32);
            }
        }
        Some(grid)
    }

    fn cube(&self, p: [f32; 3]) -> Option<[i32; 3]> {
        let c = [0, 1, 2].map(|k| ((p[k] - self.lo[k]) / self.cell).floor());
        c.iter().all(|x| x.is_finite() && x.abs() < 1e9).then(|| c.map(|x| x as i32))
    }

    /// The vertex of `at` nearest `p` among those in its cube and the cubes round it.
    fn nearest(&self, at: &[[f32; 3]], p: [f32; 3]) -> Option<usize> {
        let c = self.cube(p)?;
        let mut best: Option<(f32, u32)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(list) = self.cubes.get(&[c[0] + dx, c[1] + dy, c[2] + dz]) else { continue };
                    for &i in list {
                        let Some(q) = at.get(i as usize) else { continue };
                        let d = (0..3).map(|k| (q[k] - p[k]).powi(2)).sum::<f32>();
                        if best.is_none_or(|(e, _)| d < e) {
                            best = Some((d, i));
                        }
                    }
                }
            }
        }
        best.map(|(_, i)| i as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grid of `n` × `n` points a millimetre apart on z = 0, as flat triangles.
    fn sheet(n: u32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let positions = (0..n * n).map(|i| Vec3::new(f64::from(i % n), f64::from(i / n), 0.0)).collect();
        let mut triangles = Vec::new();
        for j in 0..n - 1 {
            for i in 0..n - 1 {
                let a = j * n + i;
                triangles.push([a, a + 1, a + n + 1]);
                triangles.push([a, a + n + 1, a + n]);
            }
        }
        (positions, triangles)
    }

    /// A coarse level of a sheet painted from it: a one-vertex spike far along the scale shows on
    /// the level vertex nearest it, not averaged away, its sign kept; the rest takes the sheet's
    /// own; a level vertex no sheet vertex is nearest is filled from its neighbours.
    #[test]
    fn a_coarse_level_keeps_a_spike_and_leaves_no_specks() {
        let (positions, _) = sheet(41);
        let mut scale = vec![0.2f32; positions.len()];
        let spike = 20 * 41 + 21;
        scale[spike] = -0.9;
        scale[0] = f32::NAN;
        // The level: every fourth point each way, and one more vertex off the sheet far away.
        let (coarse, triangles) = sheet(11);
        let mut at: Vec<Vec3> = coarse.iter().map(|p| *p * 4.0).collect();
        at.push(Vec3::new(1000.0, 1000.0, 0.0));
        let mut triangles = triangles;
        triangles.push([121, 0, 1]);
        let level = SharedTris::from_level(&solidcraft_engine::geom::simplify::Level { positions: at, triangles, error: 0.0 });
        let painted = strongest_on(&positions, &scale, &level).expect("painted");
        let nearest = 5 * 11 + 5;
        assert_eq!(painted[nearest], -0.9, "the spike at (21, 20) shows on the vertex at (20, 20)");
        assert!(painted.iter().enumerate().filter(|(i, _)| *i != nearest && *i != 121).all(|(_, s)| *s == 0.2), "{painted:?}");
        assert!(!painted[121].is_nan(), "the far vertex takes its neighbours'");
    }

    /// A scan's own colours: unpainted vertices take none (alpha 0), the rest the scale's colour; a
    /// painting that does not fit the scan's vertices paints nothing.
    #[test]
    fn a_scan_is_painted_on_its_own_vertices() {
        let (positions, triangles) = sheet(3);
        let mesh = Arc::new(Mesh { positions, triangles, ..Default::default() });
        let tris = Arc::new(SharedTris::of(&mesh));
        let colours: Colours = [[0, 0, 255], [0, 128, 255], [0, 200, 0], [255, 128, 0], [255, 0, 0]];
        let mut scale = vec![0.0f32; 9];
        scale[4] = 1.0;
        scale[8] = f32::NAN;
        let painting = Painting { scan: "Scan".into(), key: 9_000_001, scale: Arc::new(scale), colours };
        let (painted, levels) = paint(&painting, &mesh, &tris, &[]).expect("painted");
        assert!(levels.is_empty());
        let whole = &painted.levels[0];
        assert_eq!(whole[0], paint_colour([0, 200, 0]), "within: the within colour");
        assert_eq!(whole[4], paint_colour([255, 0, 0]), "the far end above");
        assert_eq!(whole[8], 0, "left unpainted");
        let short = Painting { scale: Arc::new(vec![0.0; 3]), key: 9_000_002, ..painting };
        assert!(paint(&short, &mesh, &tris, &[]).is_none());
    }
}
