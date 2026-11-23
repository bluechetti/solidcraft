//! Read-only access to the skill documents shipped with SolidCraft.

use serde_json::{Map, Value};

struct Skill {
    name: &'static str,
    files: &'static [(&'static str, &'static str)],
}

// Exact names only: these are embedded documents, never filesystem paths to open.
const SKILLS: &[Skill] = &[
    Skill {
        name: "solidcraft-checking",
        files: &[
            ("SKILL.md", include_str!("../../../skills/solidcraft-checking/SKILL.md")),
            ("reference/porting.md", include_str!("../../../skills/solidcraft-checking/reference/porting.md")),
        ],
    },
    Skill { name: "solidcraft-editing", files: &[("SKILL.md", include_str!("../../../skills/solidcraft-editing/SKILL.md"))] },
    Skill {
        name: "solidcraft-mechanisms",
        files: &[
            ("SKILL.md", include_str!("../../../skills/solidcraft-mechanisms/SKILL.md")),
            ("reference.md", include_str!("../../../skills/solidcraft-mechanisms/reference.md")),
        ],
    },
    Skill {
        name: "solidcraft-modeling",
        files: &[
            ("SKILL.md", include_str!("../../../skills/solidcraft-modeling/SKILL.md")),
            ("reference.md", include_str!("../../../skills/solidcraft-modeling/reference.md")),
        ],
    },
    Skill {
        name: "solidcraft-scan-to-cad",
        files: &[
            ("SKILL.md", include_str!("../../../skills/solidcraft-scan-to-cad/SKILL.md")),
            ("reference.md", include_str!("../../../skills/solidcraft-scan-to-cad/reference.md")),
        ],
    },
    Skill {
        name: "solidcraft-sheetmetal",
        files: &[
            ("SKILL.md", include_str!("../../../skills/solidcraft-sheetmetal/SKILL.md")),
            ("reference.md", include_str!("../../../skills/solidcraft-sheetmetal/reference.md")),
        ],
    },
];

pub(crate) fn read(args: &Map<String, Value>) -> Result<Value, String> {
    let name = args.get("skill").and_then(Value::as_str).ok_or("`skill` is required and must be a string")?;
    let skill = SKILLS
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| format!("Unknown skill. Available skills: {}", SKILLS.iter().map(|s| s.name).collect::<Vec<_>>().join(", ")))?;
    let file = match args.get("file") {
        None | Some(Value::Null) => "reference.md",
        Some(Value::String(file)) => file,
        Some(_) => return Err("`file` must be a string".into()),
    };
    let text = skill.files.iter().find(|(name, _)| *name == file).map(|(_, text)| *text).ok_or_else(|| {
        format!("Unknown file for {}. Available files: {}", skill.name, skill.files.iter().map(|(name, _)| *name).collect::<Vec<_>>().join(", "))
    })?;
    match args.get("section") {
        None | Some(Value::Null) => Ok(Value::String(text.into())),
        Some(Value::String(want)) => section(text, want).map(Value::String),
        Some(_) => Err("`section` must be a string: a heading of the file, or \"contents\"".into()),
    }
}

/// The file's `## ` sections: each heading (without the `## `) and its text, the heading line
/// included. What comes before the first heading (the title and its first lines) is not one.
fn sections(text: &str) -> Vec<(&str, &str)> {
    let mut starts: Vec<usize> = text.match_indices("\n## ").map(|(at, _)| at + 1).chain(text.starts_with("## ").then_some(0)).collect();
    starts.sort_unstable();
    starts
        .iter()
        .enumerate()
        .map(|(i, &at)| {
            let end = starts.get(i + 1).copied().unwrap_or(text.len());
            let body = &text[at..end];
            let heading = body.lines().next().unwrap_or_default().trim_start_matches("## ").trim();
            (heading, body)
        })
        .collect()
}

/// One section of a skill document, found by its heading (any case; the start of a heading or a
/// word in it is enough when only one heading matches), or `contents`: the title, then each
/// heading with its size, so an agent reads only the sections it needs (a whole reference is
/// 16–28 KB, kept in its context for the rest of the session).
fn section(text: &str, want: &str) -> Result<String, String> {
    let all = sections(text);
    let want = want.trim().trim_start_matches('#').trim().to_lowercase();
    if want == "contents" {
        let title = text.lines().take_while(|l| !l.starts_with("## ")).collect::<Vec<_>>().join("\n");
        let list: Vec<String> = all.iter().map(|(h, body)| format!("- {h} ({:.1} KB)", body.len() as f64 / 1000.0)).collect();
        return Ok(format!("{}\n\nSections (read one with `section`: its heading):\n{}", title.trim_end(), list.join("\n")));
    }
    let headings = || all.iter().map(|(h, _)| *h).collect::<Vec<_>>().join("; ");
    if let Some((_, body)) = all.iter().find(|(h, _)| h.to_lowercase() == want) {
        return Ok((*body).to_owned());
    }
    let found: Vec<&(&str, &str)> = all.iter().filter(|(h, _)| h.to_lowercase().starts_with(&want) || h.to_lowercase().contains(&want)).collect();
    match found.as_slice() {
        [(_, body)] => Ok((*body).to_owned()),
        [] => Err(format!("no section `{want}`; the sections are: {}", headings())),
        many => Err(format!("`{want}` matches several sections: {}", many.iter().map(|(h, _)| *h).collect::<Vec<_>>().join("; "))),
    }
}
