//! Inspection Report (`scan.report`): one HTML file that says what a scan is, what was done to
//! it and what was read off it, with pictures inside the file, so it opens in any browser, prints
//! to PDF from there, and needs nothing kept beside it: the scan's survey (its size, fineness,
//! noise, holes, pieces), the steps that cleaned it, the surface its scanner never saw, the
//! deviation map when one is shown, the construction features of the design, and pictures from
//! three sides (drawn by the renderer the host installed; without one the report says so).
//! It changes nothing in the design.

use serde_json::{Value, json};
use solidcraft_doc::FeatureKind;

use crate::params::bad;
use crate::schema::scan::ScanReportResult;
use crate::{EngineError, Result, Session};

/// The pictures' size, pixels.
const PICTURE: (u32, u32) = (900, 560);

/// Text made safe to put in HTML.
fn esc(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn mm(v: f64) -> String {
    format!("{v:.3} mm")
}

/// A table of label and value rows.
fn table(rows: &[(String, String)]) -> String {
    let body: String = rows.iter().map(|(k, v)| format!("<tr><th>{}</th><td>{}</td></tr>", esc(k), esc(v))).collect();
    format!("<table>{body}</table>")
}

/// The pictures of the design from three sides, as `<img>` tags with the PNGs inside, or why
/// there are none.
fn pictures(s: &Session) -> std::result::Result<Vec<(String, String)>, String> {
    use solidcraft_viewport::StandardView;
    let mut out = Vec::new();
    for (view, title) in [(StandardView::Iso, "Iso"), (StandardView::Front, "Front"), (StandardView::Top, "Top")] {
        let mut cam = crate::view::home_camera(s).map_err(|e| e.to_string())?;
        cam.set_view(view);
        let scene = crate::view::scene(s, &cam).map_err(|e| e.to_string())?;
        let png = crate::capture::png(&scene, &cam, PICTURE.0, PICTURE.1)?;
        out.push((title.to_owned(), solidcraft_doc::canvas::base64_encode(&png.bytes)));
    }
    Ok(out)
}

pub(super) fn report(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.report";
    let path = crate::params::str_(p, "path")
        .filter(|x| !x.trim().is_empty())
        .ok_or_else(|| bad(cmd, "`path`: where to write the report (an .html file)"))?;
    if !path.to_ascii_lowercase().ends_with(".html") && !path.to_ascii_lowercase().ends_with(".htm") {
        return Err(bad(cmd, "`path` must end in .html"));
    }
    let body = super::mesh_body_name(s, crate::params::str_(p, "body"), cmd)?;
    let title = crate::params::str_(p, "title").map(str::to_owned).unwrap_or_else(|| format!("{body}: inspection report"));
    let with_pictures = crate::params::bool_(p, "pictures").unwrap_or(true);
    let mut sections: Vec<(String, String)> = Vec::new();

    // What the scan is.
    let survey = crate::cmd::find_command("scan.survey")
        .ok_or_else(|| EngineError::UnknownCommand("scan.survey".into()))?
        .run_nested(s, &json!({"body": body}))?;
    let num = |k: &str| survey.get(k).and_then(Value::as_f64);
    let count = |k: &str| survey.get(k).and_then(Value::as_u64).unwrap_or(0).to_string();
    let size = survey
        .get("own_size")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_f64).map(|v| format!("{v:.1}")).collect::<Vec<_>>().join(" × "))
        .unwrap_or_default();
    let mut rows = vec![
        ("Triangles".to_owned(), count("triangles")),
        ("Points".to_owned(), count("points")),
        ("Size along its own directions".to_owned(), format!("{size} mm")),
        ("Area".to_owned(), num("area").map(|v| format!("{v:.1} mm²")).unwrap_or_default()),
        ("Typical triangle side".to_owned(), num("typical_edge").map(mm).unwrap_or_default()),
        ("Noise".to_owned(), num("noise").map(mm).unwrap_or_else(|| "not read".into())),
        ("Tolerance for fits".to_owned(), num("tolerance").map(mm).unwrap_or_default()),
        ("Holes".to_owned(), count("holes")),
        ("Pieces (floating)".to_owned(), format!("{} ({})", count("pieces"), count("floating"))),
        ("Broad flat faces".to_owned(), num("flat_share").map(|v| format!("{:.0} %", 100.0 * v)).unwrap_or_default()),
        ("Never seen by the scanner".to_owned(), num("unseen_share").map(|v| format!("{:.1} %", 100.0 * v)).unwrap_or_default()),
    ];
    if let Some(hint) = survey.get("units_hint").and_then(Value::as_str) {
        rows.push(("Units".to_owned(), hint.to_owned()));
    }
    sections.push(("The scan".into(), table(&rows)));

    // What was done to it, in timeline order.
    let steps: Vec<(String, String)> = s
        .doc
        .features
        .iter()
        .filter(|f| matches!(&f.kind, FeatureKind::ScanClean { body: b, .. } if b.0 == body) || f.made_from.contains(&body))
        .map(|f| {
            let what = match &f.kind {
                FeatureKind::ScanClean { step, .. } => {
                    // A step's settings, cut short: an area's runs can be long.
                    let settings: String = step.params().to_string().chars().take(160).collect();
                    format!("{}: {settings}", step.title())
                }
                _ => "made from the scan".to_owned(),
            };
            (f.name.clone(), if f.suppressed { format!("{what} (suppressed)") } else { what })
        })
        .collect();
    sections
        .push(("What was done to it".into(), if steps.is_empty() { "<p>Nothing: it is the scan as it came in.</p>".into() } else { table(&steps) }));

    // What its scanner never saw.
    let (source, unseen) = super::unseen::unseen_of(s, &body)?;
    let unseen_html = match unseen {
        Some(u) if u.area > 0.0 => {
            let mut rows = vec![
                ("Source".to_owned(), source.name().to_owned()),
                ("Area".to_owned(), format!("{:.1} mm² ({:.1} % of the scan)", u.area, 100.0 * u.share())),
            ];
            for (k, patch) in u.patches.iter().take(10).enumerate() {
                rows.push((
                    format!("Patch {}", k + 1),
                    format!("{:.1} mm² at ({:.1}, {:.1}, {:.1})", patch.area, patch.centre.x, patch.centre.y, patch.centre.z),
                ));
            }
            table(&rows)
        }
        _ => format!("<p>None ({}).</p>", source.name()),
    };
    sections.push(("Surface the scanner never saw".into(), unseen_html));

    // The deviation map, when one is shown for this scan.
    if let Some(map) = s.scan_deviation.clone().filter(|m| m.scan == body && m.current(s)) {
        let rows = vec![
            ("Against".to_owned(), map.body.clone()),
            ("Root mean square".to_owned(), mm(map.rms)),
            ("Within tolerance".to_owned(), format!("{:.1} % (±{})", 100.0 * map.within, mm(map.tolerance))),
            ("Passes (95 % within)".to_owned(), if map.passes() { "yes".into() } else { "no".into() }),
        ];
        sections.push(("Deviation".into(), table(&rows)));
    }

    // The construction features: planes, axes and points kept from fits.
    let kept: Vec<(String, String)> = s
        .doc
        .features
        .iter()
        .filter(|f| {
            matches!(f.kind, FeatureKind::ConstructionPlane { .. } | FeatureKind::ConstructionAxis { .. } | FeatureKind::ConstructionPoint { .. })
        })
        .map(|f| (f.name.clone(), if f.suppressed { "suppressed".into() } else { "kept".into() }))
        .collect();
    if !kept.is_empty() {
        sections.push(("Construction features".into(), table(&kept)));
    }

    // Pictures.
    let (pictures, why) = if with_pictures {
        match pictures(s) {
            Ok(p) => (p, None),
            Err(e) => (Vec::new(), Some(e)),
        }
    } else {
        (Vec::new(), Some("not asked for".to_owned()))
    };
    let mut figures: String = pictures
        .iter()
        .map(|(t, b64)| format!("<figure><img alt=\"{t}\" src=\"data:image/png;base64,{b64}\"><figcaption>{t}</figcaption></figure>"))
        .collect();
    if let Some(why) = &why {
        figures = format!("<p>No pictures: {}.</p>", esc(why));
    }
    sections.push(("Pictures".into(), figures));

    let made = format!("{} (SolidCraft {})", esc(&s.doc.name), env!("CARGO_PKG_VERSION"));
    let body_html: String = sections.iter().map(|(h, html)| format!("<section><h2>{}</h2>{html}</section>", esc(h))).collect();
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{t}</title><style>body{{font:15px/1.45 system-ui,sans-serif;margin:24px auto;max-width:960px;padding:0 16px;color:#222;background:#fff}}h1{{font-size:24px}}h2{{font-size:18px;margin-top:28px;border-bottom:1px solid #ccc}}table{{border-collapse:collapse;width:100%}}th,td{{text-align:left;padding:4px 8px;border-bottom:1px solid #eee;vertical-align:top}}th{{width:40%;font-weight:600}}figure{{margin:12px 0}}img{{max-width:100%;border:1px solid #ddd}}@media print{{section{{break-inside:avoid}}}}</style></head><body><h1>{t}</h1><p>{made}</p>{body_html}</body></html>",
        t = esc(&title)
    );
    solidcraft_io::vfs::write(path, html.as_bytes()).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    let reply = ScanReportResult {
        path: path.to_owned(),
        body,
        bytes: html.len(),
        sections: sections.iter().map(|(h, _)| h.clone()).collect(),
        pictures: pictures.len(),
        no_pictures: why,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}
