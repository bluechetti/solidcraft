//! The prompts SolidCraft serves (MCP `prompts/list`, `prompts/get`): a ready start for the work
//! people most often bring, which a client offers as a slash command. Each fills in the user's
//! words and points the agent at the bundled skill that holds the workflow, so the steps live in
//! one place (`skills/`) and the prompt stays a few lines.

use serde_json::{Value, json};

/// Each prompt: its name, title, description, arguments (name, description, required) and the
/// text it becomes, with `{argument}` where an argument's words go.
struct Prompt {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    arguments: &'static [(&'static str, &'static str, bool)],
    text: &'static str,
}

const PROMPTS: &[Prompt] = &[
    Prompt {
        name: "edit_design",
        title: "Change a design",
        description: "Change an existing design through its parameters, keeping it parametric",
        arguments: &[("change", "What to change, in your words", true), ("path", "The design file (default: the one open)", false)],
        text: "Change the SolidCraft design{path}: {change}\n\n\
Call `skill_reference {\"skill\":\"solidcraft-editing\"}` first and follow it: map the design with `inspect_design {\"measure\":false,\"sketches\":false}` and `measure {}`, change parameters rather than rebuilding, predict the volume change and check it with one `measure {}`. If the design holds a scan (a mesh body), check against it with `scan.deviation {}` as well. Tell me what changed and which reading of my words you used.",
    },
    Prompt {
        name: "model_part",
        title: "Model a part",
        description: "Build a new parametric part from a description",
        arguments: &[("part", "The part: what it is, its sizes and features", true)],
        text: "Model this part in SolidCraft: {part}\n\n\
Call `skill_reference {\"skill\":\"solidcraft-modeling\"}` first and follow it: run `inspect_design {\"measure\":false,\"sketches\":false}`, write the parameters, features and expected volume and bounding box before the first change, build in a few `batch` calls, and check each stage with `measure {}` against your numbers. Name every feature and keep every size a parameter. Finish with one `screenshot {}` and tell me the parameters I can change.",
    },
    Prompt {
        name: "scan_to_cad",
        title: "Scan to CAD",
        description: "Turn a scan (STL, PLY or OBJ mesh) into a parametric CAD model checked against it",
        arguments: &[
            ("path", "The scan file (default: the scan already in the design)", false),
            ("accuracy", "How close the model must follow the scan, in mm (default: about three times its noise)", false),
            ("goal", "What the model is for: a part to machine, a wall template, a surface to print over…", false),
        ],
        text: "Turn the scan{path} into CAD in SolidCraft{goal}.{accuracy}\n\n\
Call `skill_reference {\"skill\":\"solidcraft-scan-to-cad\"}` first and follow it: read the scan with `scan.survey {}` and its `next`, clean and place it, choose the route by the kind of part, and check each stage with `scan.deviation {}`. Sizes read off the scan become parameters (rounded to design values, the measured value in the comment). Stop when `scan.deviation {}` says `passes`, or tell me which regions still miss and by how much.",
    },
];

/// Every prompt's text, for the test that checks they name only real tools and commands.
#[cfg(test)]
pub(crate) fn texts() -> Vec<&'static str> {
    PROMPTS.iter().map(|p| p.text).collect()
}

/// `prompts/list`.
pub(crate) fn list() -> Vec<Value> {
    PROMPTS
        .iter()
        .map(|p| {
            let arguments: Vec<Value> = p
                .arguments
                .iter()
                .map(|(name, description, required)| json!({"name": name, "description": description, "required": required}))
                .collect();
            json!({"name": p.name, "title": p.title, "description": p.description, "arguments": arguments})
        })
        .collect()
}

/// `prompts/get`: prompt `name` with `args` put in (the server checked them against the listing:
/// strings, each required one there).
pub(crate) fn get(name: &str, args: &Value) -> Result<Value, String> {
    let prompt = PROMPTS.iter().find(|p| p.name == name).ok_or_else(|| format!("no prompt `{name}`"))?;
    let given = |key: &str| args.get(key).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty());
    let mut text = prompt.text.to_string();
    for (key, _, _) in prompt.arguments {
        // An optional argument left out leaves no trace in the sentence.
        let filled = match (*key, given(key)) {
            ("path", Some(v)) if name == "edit_design" => format!(" `{v}` (open it with `open {{\"path\":\"{v}\"}}` first)"),
            ("path", Some(v)) => format!(" `{v}` (insert it with `file.insert_mesh {{\"path\":\"{v}\"}}` if it is not in the design yet)"),
            ("path", None) if name == "edit_design" => String::new(),
            ("path", None) => " already in the design".to_string(),
            ("accuracy", Some(v)) => match number(v) {
                Some(mm) => format!(" The model must follow it within {mm} mm: set that with `scan.tolerance {{\"tolerance\": {mm}}}`."),
                None => format!(" The model must follow it within {v}: set that with `scan.tolerance {{}}`."),
            },
            ("goal", Some(v)) => format!(", for {v}"),
            (_, Some(v)) => v.to_string(),
            (_, None) => String::new(),
        };
        text = text.replace(&format!("{{{key}}}"), &filled);
    }
    Ok(json!({"description": prompt.description, "messages": [{"role": "user", "content": {"type": "text", "text": text}}]}))
}

/// `v` as millimetres when it reads as a length in them ("0.2", "0.2 mm"), so it goes into the
/// call as a number; `None` for anything else, which the sentence then quotes as typed.
fn number(v: &str) -> Option<String> {
    let bare = v.trim().trim_end_matches("mm").trim();
    bare.parse::<f64>().ok().filter(|x| x.is_finite() && *x > 0.0).map(|_| bare.to_string())
}
