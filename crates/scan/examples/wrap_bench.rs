//! Wrap Scan alone on a binary STL: time, peak memory, the coarse mesh's rounds and both-way
//! distances, the patches and their distance from the scan.
//! `cargo run --example wrap_bench -p solidcraft-scan --profile fast -- scan.stl [tolerance] [max_patches]`
use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;
use solidcraft_scan::Scan;
use solidcraft_scan::wrap_scan::{Settings, wrap};

fn peak_gb() -> f64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    status.lines().find(|l| l.starts_with("VmHWM:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0)
        / 1_048_576.0
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let tolerance: f64 = args.get(2).and_then(|t| t.parse().ok()).unwrap_or(0.2);
    let max_patches: usize = args.get(3).and_then(|t| t.parse().ok()).unwrap_or(12_000);
    let data = std::fs::read(&args[1]).expect("read the STL");
    let n = u32::from_le_bytes(data[80..84].try_into().expect("header")) as usize;
    let mut index = std::collections::HashMap::new();
    let mut positions = Vec::new();
    let mut triangles = Vec::with_capacity(n);
    for t in 0..n {
        let base = 84 + 50 * t + 12;
        let mut tri = [0u32; 3];
        for (k, slot) in tri.iter_mut().enumerate() {
            let f = |o: usize| f32::from_le_bytes(data[base + 12 * k + o..base + 12 * k + o + 4].try_into().expect("f32"));
            let key = [f(0).to_bits(), f(4).to_bits(), f(8).to_bits()];
            *slot = *index.entry(key).or_insert_with(|| {
                positions.push(Vec3::new(f(0) as f64, f(4) as f64, f(8) as f64));
                (positions.len() - 1) as u32
            });
        }
        triangles.push(tri);
    }
    let t0 = std::time::Instant::now();
    let scan = Scan::new(&positions, &triangles);
    match wrap(&scan, &Settings { tolerance, max_patches, ..Settings::default() }) {
        Ok(w) => {
            let c = &w.coarse;
            println!("triangles {} -> coarse {} in {} rounds (met {}, capped {})", triangles.len(), c.triangles.len(), c.rounds, c.met, c.capped);
            let shape = |t: &[u32; 3]| {
                let q = t.map(|i| c.positions[i as usize]);
                let area2 = (q[1] - q[0]).cross(q[2] - q[0]).len();
                let sides = (q[1] - q[0]).dot(q[1] - q[0]) + (q[2] - q[1]).dot(q[2] - q[1]) + (q[0] - q[2]).dot(q[0] - q[2]);
                if sides > 0.0 { 2.0 * 3f64.sqrt() * area2 / sides } else { 0.0 }
            };
            let shapes: Vec<f64> = c.triangles.iter().map(shape).collect();
            println!(
                "coarse shapes: below 0.1 {}, below 0.35 {}, worst {:.4}",
                shapes.iter().filter(|&&q| q < 0.1).count(),
                shapes.iter().filter(|&&q| q < 0.35).count(),
                shapes.iter().copied().fold(1.0, f64::min)
            );
            println!(
                "patches {}; scan->patches within {tolerance}: {:.4} p99 {:.3} max {:.3}; {:.1} s, peak {:.2} GB",
                w.faces.len(),
                w.scan_to_patches.within,
                w.scan_to_patches.p99,
                w.scan_to_patches.max,
                t0.elapsed().as_secs_f64(),
                peak_gb()
            );
            let mut uses = std::collections::HashMap::new();
            for t in &c.triangles {
                for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                    *uses.entry((a.min(b), a.max(b))).or_insert(0usize) += 1;
                }
            }
            let smallest_angle = c.triangles.iter().map(|t| {
                let q = t.map(|i| c.positions[i as usize]);
                (0..3).map(|k| {
                    let (u, v) = (q[(k + 1) % 3] - q[k], q[(k + 2) % 3] - q[k]);
                    (u.dot(v) / (u.len() * v.len()).max(1e-300)).clamp(-1.0, 1.0).acos().to_degrees()
                }).fold(180.0, f64::min)
            }).fold(180.0, f64::min);
            println!(
                "coarse edges: open {}, of 3+ faces {}; smallest angle {:.2} deg",
                uses.values().filter(|&&n| n == 1).count(),
                uses.values().filter(|&&n| n > 2).count(),
                smallest_angle
            );
            let (mut lo, mut hi) = (180.0f64, 0.0f64);
            let mut bad = [0usize; 4];
            for f in &w.faces {
                let net = solidcraft_scan::surface_edit::points(f);
                let (mut plo, mut phi) = (180.0f64, 0.0f64);
                for (o, p, q) in [(0, 1, 4), (3, 2, 7), (15, 14, 11), (12, 13, 8)] {
                    let (u, v) = (net[p] - net[o], net[q] - net[o]);
                    let a = (u.dot(v) / (u.len() * v.len()).max(1e-300)).clamp(-1.0, 1.0).acos().to_degrees();
                    plo = plo.min(a);
                    phi = phi.max(a);
                }
                lo = lo.min(plo);
                hi = hi.max(phi);
                bad[0] += usize::from(plo < 5.0);
                bad[1] += usize::from(plo < 10.0);
                bad[2] += usize::from(phi > 170.0);
                bad[3] += usize::from(phi > 175.0);
            }
            println!("patch corners: smallest {lo:.1} deg, largest {hi:.1} deg; patches with a corner <5 {} <10 {} >170 {} >175 {}", bad[0], bad[1], bad[2], bad[3]);
            let coarse_tree = solidcraft_geom::compare::TriangleTree::of(&c.positions, &c.triangles);
            for (p, d) in w.worst.iter().take(4) {
                let to_coarse = coarse_tree.nearest(*p).map_or(f64::NAN, |(d, _)| d);
                println!("  worst {d:.3} at ({:.2}, {:.2}, {:.2}); to the coarse mesh {to_coarse:.3}", p.x, p.y, p.z);
            }
        }
        Err(e) => println!("refused: {e}"),
    }
}
