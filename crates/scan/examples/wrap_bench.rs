//! Wrap Scan alone on a binary STL: time, peak memory, the coarse mesh's rounds and both-way
//! distances, the patches and their distance from the scan.
//! `cargo run --example wrap_bench -p solidcraft-scan --profile fast -- scan.stl [tolerance] [max_patches]`
use solidcore_record::Vec3;
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
            println!(
                "patches {}; scan->patches within {tolerance}: {:.4} p99 {:.3} max {:.3}; {:.1} s, peak {:.2} GB",
                w.faces.len(),
                w.scan_to_patches.within,
                w.scan_to_patches.p99,
                w.scan_to_patches.max,
                t0.elapsed().as_secs_f64(),
                peak_gb()
            );
        }
        Err(e) => println!("refused: {e}"),
    }
}
