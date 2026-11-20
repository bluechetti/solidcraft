//! Find All's time and peak memory on a binary STL, alone (no app, no survey), then Reconstruct's
//! reading of it; with `profile`, every face found and each prism's profile as drawn. For huge
//! scans and for replaying one read: `cargo run --example find_all_bench -p solidcraft-scan
//! --profile <p> -- scan.stl [profile]` (a profile inheriting `release` reads 92M triangles in
//! about four minutes).
use solidcore_record::Vec3;
use solidcraft_scan::Scan;

fn peak_gb() -> f64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let kb = |key: &str| {
        status.lines().find(|l| l.starts_with(key)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0)
    };
    kb("VmHWM:") / 1_048_576.0
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let t0 = std::time::Instant::now();
    let data = std::fs::read(&args[1]).unwrap();
    let n = u32::from_le_bytes(data[80..84].try_into().unwrap()) as usize;
    // Weld by sorting the corners' bit patterns (no hash map: 3n corners at 92M).
    let mut keys: Vec<([u32; 3], u32)> = Vec::with_capacity(3 * n);
    for t in 0..n {
        let base = 84 + 50 * t + 12;
        for k in 0..3 {
            let f = |o: usize| f32::from_le_bytes(data[base + 12 * k + o..base + 12 * k + o + 4].try_into().unwrap()).to_bits();
            keys.push(([f(0), f(4), f(8)], (3 * t + k) as u32));
        }
    }
    drop(data);
    keys.sort_unstable();
    let mut positions = Vec::new();
    let mut corner = vec![0u32; 3 * n];
    let mut last = None;
    for (key, c) in &keys {
        if last != Some(*key) {
            positions.push(Vec3::new(f64::from(f32::from_bits(key[0])), f64::from(f32::from_bits(key[1])), f64::from(f32::from_bits(key[2]))));
            last = Some(*key);
        }
        corner[*c as usize] = (positions.len() - 1) as u32;
    }
    drop(keys);
    let triangles: Vec<[u32; 3]> = (0..n).map(|t| [corner[3 * t], corner[3 * t + 1], corner[3 * t + 2]]).collect();
    drop(corner);
    eprintln!("loaded {} tris, {} points in {:.1} s, peak {:.2} GB", triangles.len(), positions.len(), t0.elapsed().as_secs_f64(), peak_gb());
    let scan = Scan::new(&positions, &triangles);
    if args.get(2).map(String::as_str) == Some("survey") {
        // Each of Survey's passes on its own: its time and the most memory it took over what was
        // held before it (the high-water mark reset before each).
        let held = || {
            let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
            status
                .lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.0)
                / 1_048_576.0
        };
        let part = |name: &str, f: &dyn Fn()| {
            let _ = std::fs::write("/proc/self/clear_refs", "5");
            let (before, t0) = (held(), std::time::Instant::now());
            f();
            eprintln!("{name:<16} {:>6.1} s  peak +{:.2} GB", t0.elapsed().as_secs_f64(), peak_gb() - before);
        };
        part("typical edge", &|| {
            std::hint::black_box(solidcraft_scan::remesh::typical_edge(&scan));
        });
        part("noise", &|| {
            std::hint::black_box(solidcraft_scan::survey::noise(&scan));
        });
        part("holes", &|| {
            std::hint::black_box(solidcraft_scan::holes::survey(&scan));
        });
        part("pieces", &|| {
            std::hint::black_box(solidcraft_scan::pieces::pieces(&scan));
        });
        part("bases", &|| {
            std::hint::black_box(solidcraft_scan::base::bases(&scan));
        });
        part("crossings", &|| {
            std::hint::black_box(solidcraft_scan::repair::crossing_count(&scan));
        });
        part("whole survey", &|| {
            std::hint::black_box(solidcraft_scan::survey::look(&scan, 0.01));
        });
        return;
    }
    let options = solidcraft_scan::segment::Options::default();
    let t0 = std::time::Instant::now();
    let out = solidcraft_scan::segment::find_all(&scan, &options, &|| false);
    let shapes = out.segments.iter().filter(|s| s.shape.is_some()).count();
    eprintln!(
        "find_all {:.1} s, peak {:.2} GB, {} shapes, in_shapes {:.3}, noise {:.4}, tolerance {:.4}",
        t0.elapsed().as_secs_f64(),
        peak_gb(),
        shapes,
        out.in_shapes,
        out.noise,
        out.tolerance
    );
    let t0 = std::time::Instant::now();
    let reading = solidcraft_scan::reconstruct::read(&scan, &out);
    eprintln!("reconstruct::read {:.1} s, peak {:.2} GB, {} faces", t0.elapsed().as_secs_f64(), peak_gb(), reading.faces.len());
    if args.get(2).map(String::as_str) == Some("profile") {
        for (k, face) in reading.faces.iter().enumerate() {
            eprintln!("face {k}: {:?}", face.shape);
        }
        for (k, prism) in reading.prisms.iter().enumerate() {
            eprintln!("prism {k}: walls {:?} ends {:?} low {:.3} high {:.3}", prism.walls, prism.ends, prism.low, prism.high);
            match solidcraft_scan::profile::prism_profile(&scan, &out, &reading, prism) {
                Ok((_, outlines)) => {
                    for o in outlines {
                        for i in 0..o.corners.len() {
                            eprintln!("   {:>8.3} {:>8.3}  {:?} face {:?}", o.corners[i].x, o.corners[i].y, o.pieces[i], o.faces[i]);
                        }
                        eprintln!("   --");
                    }
                }
                Err(e) => eprintln!("   refused: {e}"),
            }
        }
    }
}
