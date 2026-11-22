//! A bicubic patch network from a file, sewn with `sew_network` and meshed at a chord share of its
//! size: says whether it meshes and, when a face does not, prints that face and its neighbours (the
//! nets, the corners by chords and by tangents, the sides). For tracing a wrap the mesher refuses.
//!
//! ```sh
//! cargo run -p solidcraft-kernel --example network_mesh -- nets.txt [chord share, default 2e-4]
//! ```
//!
//! The file has one patch a line: its 16 control points (u fastest), x y z each, as Wrap Scan's
//! `wrap_bench` example writes them with `WRAP_DUMP`. With `NETWORK_RECORD=path` the sewn body's
//! record is written there as JSON (a kernel fixture's form, before gzip).

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;
use solidcraft_kernel::{EdgeSpec, FaceSpec, MeshSpec, SurfSpec, sew_network};

type Net = [Vec3; 16];

fn main() {
    if let Err(e) = run() {
        eprintln!("network_mesh: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).ok_or("usage: network_mesh nets.txt [chord share]")?;
    let share: f64 = match args.get(2) {
        Some(s) => s.parse().map_err(|e| format!("chord share: {e}"))?,
        None => 2e-4,
    };
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let mut nets: Vec<Net> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let v: Vec<f64> = line.split_whitespace().map(str::parse).collect::<Result<_, _>>().map_err(|e| format!("line {}: {e}", i + 1))?;
        if v.len() != 48 {
            return Err(format!("line {}: {} numbers, not 48", i + 1, v.len()));
        }
        nets.push(std::array::from_fn(|k| Vec3::new(v[3 * k], v[3 * k + 1], v[3 * k + 2])));
    }
    let faces: Vec<FaceSpec> = nets.iter().map(face).collect();
    let body = sew_network(&faces).map_err(|e| format!("sew: {e}"))?;
    if let Some(out) = std::env::var_os("NETWORK_RECORD") {
        let json = serde_json::to_string(body.record()).map_err(|e| format!("record: {e}"))?;
        std::fs::write(&out, json).map_err(|e| format!("record: {e}"))?;
    }
    let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
    for p in nets.iter().flatten() {
        for (k, x) in [p.x, p.y, p.z].into_iter().enumerate() {
            lo[k] = lo[k].min(x);
            hi[k] = hi[k].max(x);
        }
    }
    let size = Vec3::new(hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]).len();
    let spec = MeshSpec::chord(share * size);
    // NETWORK_EACH=1: every patch meshed alone at the same chord, the ten slowest named.
    if std::env::var_os("NETWORK_EACH").is_some() {
        let mut times: Vec<(f64, usize)> = Vec::new();
        for (i, net) in nets.iter().enumerate() {
            let t = std::time::Instant::now();
            let ok = sew_network(&[face(net)]).map_err(|e| e.to_string()).and_then(|b| b.export_mesh(&spec).map_err(|e| e.to_string())).is_ok();
            times.push((t.elapsed().as_secs_f64(), if ok { i } else { usize::MAX - i }));
        }
        // NETWORK_EACH_OUT=path: every patch's time alone, `index seconds ok` a line.
        if let Some(out) = std::env::var_os("NETWORK_EACH_OUT") {
            let text: String = times
                .iter()
                .map(|&(t, i)| if i > nets.len() { format!("{} {t:.4} 0\n", usize::MAX - i) } else { format!("{i} {t:.4} 1\n") })
                .collect();
            std::fs::write(&out, text).map_err(|e| format!("each out: {e}"))?;
        }
        let total: f64 = times.iter().map(|t| t.0).sum();
        times.sort_by(|a, b| b.0.total_cmp(&a.0));
        println!("alone: {total:.1} s in all");
        for &(t, i) in times.iter().take(10) {
            let (i, ok) = if i > nets.len() { (usize::MAX - i, false) } else { (i, true) };
            println!("  {t:.2} s{}", if ok { "" } else { " (fails)" });
            describe("   ", i, &nets);
        }
    }
    let started = std::time::Instant::now();
    let meshed = body.export_mesh(&spec);
    let seconds = started.elapsed().as_secs_f64();
    match meshed {
        Ok(_) => println!("{} patches, size {size:.1}: meshes in {seconds:.1} s", nets.len()),
        Err(e) => {
            let msg = e.to_string();
            println!("{} patches, size {size:.1}: {msg} ({seconds:.1} s)", nets.len());
            // The body's faces are in the order given.
            let failed = msg.split("face ").nth(1).and_then(|x| x.split(|c: char| !c.is_ascii_digit()).next()).and_then(|x| x.parse::<usize>().ok());
            if let Some(fi) = failed {
                describe("failed", fi, &nets);
                for n in neighbours(fi, &nets) {
                    describe("  beside it", n, &nets);
                }
                if let Some(net) = nets.get(fi) {
                    let alone = sew_network(&[face(net)]).map_err(|e| e.to_string())?.export_mesh(&spec);
                    println!("  alone it {}", if alone.is_ok() { "meshes" } else { "fails too" });
                }
            }
        }
    }
    Ok(())
}

/// The patch as a face to sew: its net and its four sides, counter-clockwise about its normal.
fn face(net: &Net) -> FaceSpec {
    let side = |idx: [usize; 4]| EdgeSpec::Chain { degree: 3, ctrl: idx.iter().map(|&i| net[i]).collect::<Vec<_>>().into() };
    FaceSpec {
        loops: vec![vec![side([0, 1, 2, 3]), side([3, 7, 11, 15]), side([15, 14, 13, 12]), side([12, 8, 4, 0])]],
        surface: SurfSpec::Bicubic { net: Box::new(*net) },
        probe: None,
    }
}

/// The patches sharing a side with patch `fi` (two corners in common).
fn neighbours(fi: usize, nets: &[Net]) -> Vec<usize> {
    let Some(me) = nets.get(fi) else { return Vec::new() };
    let corners = |n: &Net| [n[0], n[3], n[15], n[12]];
    let mine = corners(me);
    (0..nets.len()).filter(|&j| j != fi && corners(&nets[j]).iter().filter(|c| mine.iter().any(|m| m.dist(**c) < 1e-9)).count() >= 2).collect()
}

fn describe(what: &str, fi: usize, nets: &[Net]) {
    let Some(n) = nets.get(fi) else { return };
    let angle = |o: usize, p: usize, q: usize| {
        let (u, v) = (n[p] - n[o], n[q] - n[o]);
        (u.dot(v) / (u.len() * v.len()).max(1e-300)).clamp(-1.0, 1.0).acos().to_degrees()
    };
    let chords = [angle(0, 3, 12), angle(3, 15, 0), angle(15, 12, 3), angle(12, 0, 15)];
    let tangents = [angle(0, 1, 4), angle(3, 2, 7), angle(15, 14, 11), angle(12, 13, 8)];
    let sides = [n[0].dist(n[3]), n[3].dist(n[15]), n[15].dist(n[12]), n[12].dist(n[0])];
    println!(
        "{what} patch {fi}: corners by chords {chords:.1?}, by tangents {tangents:.1?}, sides {sides:.3?}, first corner {:.3?}",
        [n[0].x, n[0].y, n[0].z]
    );
}
