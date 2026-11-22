//! Not committed: Flatten's speed on a region of a real scan (binary STL).
use solidcore_record::Vec3;
use solidcraft_scan::Scan;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data = std::fs::read(&args[1]).unwrap();
    let n = u32::from_le_bytes(data[80..84].try_into().unwrap()) as usize;
    let mut positions = Vec::with_capacity(3 * n);
    let mut triangles = Vec::with_capacity(n);
    let mut index: std::collections::HashMap<[u32; 3], u32> = std::collections::HashMap::new();
    for t in 0..n {
        let base = 84 + 50 * t + 12;
        let mut tri = [0u32; 3];
        for k in 0..3 {
            let f = |o: usize| f32::from_le_bytes(data[base + 12 * k + o..base + 12 * k + o + 4].try_into().unwrap());
            let key = [f(0).to_bits(), f(4).to_bits(), f(8).to_bits()];
            tri[k] = *index.entry(key).or_insert_with(|| {
                positions.push(Vec3::new(f64::from(f(0)), f64::from(f(4)), f64::from(f(8))));
                (positions.len() - 1) as u32
            });
        }
        triangles.push(tri);
    }
    eprintln!("loaded {} tris", triangles.len());
    let scan = Scan::new(&positions, &triangles);
    let (lo, hi) = scan.bounds().unwrap();
    let middle = (lo + hi) * 0.5;
    let at = Vec3::new(middle.x, middle.y, hi.z);
    let t0 = std::time::Instant::now();
    let seed = solidcraft_scan::base::nearest_triangle(&scan, at).unwrap();
    eprintln!("seed {:.2} s", t0.elapsed().as_secs_f64());
    for radius in [12.0, 25.0, 50.0] {
        let t0 = std::time::Instant::now();
        let region = solidcraft_scan::fit::around(&scan, seed, at, radius);
        eprintln!("region {radius}: {} tris in {:.2} s", region.len(), t0.elapsed().as_secs_f64());
        for relax in [false, true] {
            let t0 = std::time::Instant::now();
            let flat = solidcraft_scan::flatten::flatten(&scan, &region, relax);
            eprintln!("radius {radius} tris {} relax {relax}: {:.2} s {:?}", region.len(), t0.elapsed().as_secs_f64(), flat.map(|f| f.stats));
        }
    }
}
