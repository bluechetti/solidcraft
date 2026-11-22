use serde_json::{Value, json};
use solidcore_record::Vec3;

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

/// A scan of the block as made: its top 0.05 high, its hole Ø10.16 with its axis 0.02 off in X.
fn made_stl() -> String {
    let mut stl = String::from("solid made\n");
    let mut facet = |a: Vec3, b: Vec3, c: Vec3| {
        stl += &format!(
            "facet normal 0 0 0\nouter loop\nvertex {} {} {}\nvertex {} {} {}\nvertex {} {} {}\nendloop\nendfacet\n",
            a.x, a.y, a.z, b.x, b.y, b.z, c.x, c.y, c.z
        );
    };
    let (hole, r) = (Vec3::new(20.02, 20.0, 0.0), 5.08);
    for (z, up) in [(10.05, true), (0.0, false)] {
        for i in 0..60 {
            for j in 0..40 {
                let p = |x: usize, y: usize| Vec3::new(x as f64, y as f64, z);
                let middle = Vec3::new(i as f64 + 0.5, j as f64 + 0.5, 0.0);
                if (middle - Vec3::new(20.0, 20.0, 0.0)).len2().sqrt() < 6.5 {
                    continue;
                }
                let (a, b, c, d) = (p(i, j), p(i + 1, j), p(i + 1, j + 1), p(i, j + 1));
                if up {
                    facet(a, b, c);
                    facet(a, c, d);
                } else {
                    facet(a, c, b);
                    facet(a, d, c);
                }
            }
        }
    }
    let ring = |k: usize, z: f64| {
        let th = std::f64::consts::TAU * k as f64 / 90.0;
        hole + Vec3::new(r * th.cos(), r * th.sin(), z)
    };
    for i in 0..20 {
        let (z0, z1) = (0.5 * i as f64, 0.5 * (i + 1) as f64);
        for k in 0..90 {
            facet(ring(k, z0), ring(k, z1), ring(k + 1, z1));
            facet(ring(k, z0), ring(k + 1, z1), ring(k + 1, z0));
        }
    }
    stl + "endsolid made\n"
}

use solidcraft_geom::prelude::*;

/// The plan read on a block with a hole: a diameter, the hole's position, the top's flatness, the
/// block's height and a point; measured on a scan of the block as made, each check reads the
/// error planted in it, the report says so, and taking a check out is undone like any edit.
#[test]
fn a_plan_measures_each_check_on_the_scan() {
    let dir = std::env::temp_dir().join(format!("solidcraft-plan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"body_name": "Block", "operation": "new", "length": 60, "width": 40, "height": 10}));
    run(&mut s, "solid.hole", json!({"position": [20, 20, 10], "diameter": 10}));
    let err = s.execute("inspect.check_measure", &json!({})).unwrap_err().to_string();
    assert!(err.contains("no checks"), "{err}");

    let face = |p: [f64; 3]| json!({"body": "Block", "point": p});
    let dia = run(&mut s, "inspect.check_add", json!({"kind": "diameter", "face": face([25.0, 20.0, 5.0]), "tolerance": 0.1, "name": "Bore"}));
    assert!((dia["nominal"].as_f64().unwrap() - 10.0).abs() < 1e-9, "{dia}");
    let pos = run(&mut s, "inspect.check_add", json!({"kind": "position", "face": face([25.0, 20.0, 5.0])}));
    assert!((pos["plus"].as_f64().unwrap() - 0.4).abs() < 1e-9 && pos["minus"] == 0.0, "a zone, twice the medium class for Ø10: {pos}");
    run(&mut s, "inspect.check_add", json!({"kind": "flatness", "face": face([50.0, 30.0, 10.0])}));
    let height = run(&mut s, "inspect.check_add", json!({"kind": "distance", "face": face([50.0, 30.0, 10.0]), "face2": face([50.0, 30.0, 0.0])}));
    assert!((height["nominal"].as_f64().unwrap() - 10.0).abs() < 1e-9 && (height["plus"].as_f64().unwrap() - 0.2).abs() < 1e-9, "{height}");
    run(&mut s, "inspect.check_add", json!({"kind": "point", "face": face([50.0, 30.0, 10.0]), "tolerance": 0.03}));
    assert_eq!(s.doc.checks.len(), 5);
    let auto = run(&mut s, "inspect.check_add", json!({"kind": "auto", "face": face([25.0, 20.0, 5.0])}));
    assert_eq!(auto["kind"], "diameter", "a round face: {auto}");
    run(&mut s, "inspect.check_remove", json!({"check": auto["check"]}));

    let err = s.execute("inspect.check_add", &json!({"kind": "diameter", "face": face([50.0, 30.0, 10.0])})).unwrap_err().to_string();
    assert!(err.contains("not round"), "{err}");
    let err = s
        .execute("inspect.check_add", &json!({"kind": "distance", "face": face([50.0, 30.0, 10.0]), "face2": face([60.0, 20.0, 5.0])}))
        .unwrap_err()
        .to_string();
    assert!(err.contains("not parallel"), "{err}");
    let err = s.execute("inspect.check_add", &json!({"kind": "flatness", "face": face([50.0, 30.0, 10.0]), "plus": 0.1})).unwrap_err().to_string();
    assert!(err.contains("zone"), "{err}");

    let path = dir.join("made.stl");
    std::fs::write(&path, made_stl()).unwrap();
    run(&mut s, "file.insert_mesh", json!({"path": path}));
    let (html, csv) = (dir.join("report.html"), dir.join("report.csv"));
    let r = run(&mut s, "inspect.check_measure", json!({"html": html, "csv": csv}));
    let checks = r["checks"].as_array().unwrap();
    let actual = |i: usize| checks[i]["actual"].as_f64().unwrap_or_else(|| panic!("check {i} unmeasured: {r}"));
    assert!((actual(0) - 10.16).abs() < 0.01, "the bore: {r}");
    assert_eq!(checks[0]["pass"], false, "0.16 over a ±0.1 bore");
    assert!((actual(1) - 0.04).abs() < 0.01 && checks[1]["pass"] == true, "the axis 0.02 off: {r}");
    assert!(actual(2) < 0.01 && checks[2]["pass"] == true, "the top is flat: {r}");
    assert!((actual(3) - 10.05).abs() < 0.005 && checks[3]["pass"] == true, "the height: {r}");
    assert!((actual(4) - 0.05).abs() < 0.005 && checks[4]["pass"] == false, "the top 0.05 high against ±0.03: {r}");
    assert_eq!((r["passed"].as_u64(), r["failed"].as_u64(), r["unmeasured"].as_u64()), (Some(3), Some(2), Some(0)), "{r}");
    let page = std::fs::read_to_string(&html).unwrap();
    assert!(page.contains("3 of 5 checks pass") && page.contains("Bore"), "{page}");
    assert_eq!(std::fs::read_to_string(&csv).unwrap().lines().count(), 6, "a header and a line a check");
    assert!(s.inspection.as_ref().is_some_and(|i| i.current(&s) && i.results.len() == 5));

    let first = s.doc.checks[0].id;
    let gone = run(&mut s, "inspect.check_remove", json!({"check": first}));
    assert_eq!(gone["left"], 4);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.doc.checks.len(), 5, "undone");
    assert_eq!(run(&mut s, "inspect.check_list", json!({}))["checks"].as_array().unwrap().len(), 5);
    assert_eq!(run(&mut s, "inspect.check_remove", json!({"all": true}))["left"], 0);
    let _ = std::fs::remove_dir_all(&dir);
}
