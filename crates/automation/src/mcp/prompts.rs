//! MCP prompts: workflows any client can start. Each is a skill file in the
//! repository's `skills/` folder, served without its front matter, so an
//! agent reading the skill and one using the prompt follow the same text.
//! Only what today's server needs; the planned MCP surface serves the same
//! file as a skill, a prompt and through `describe`.
use serde_json::{Value, json};

/// The datasheet-to-component workflow (`skills/pcbcraft-datasheet-part`).
pub const DATASHEET_PART_SKILL: &str =
    include_str!("../../../../skills/pcbcraft-datasheet-part/SKILL.md");
/// Its prompt name.
pub const DATASHEET_PART: &str = "datasheet_to_component";
/// Bytes of one prompt argument.
const MAX_ARGUMENT: usize = 16 * 1024;

/// A skill file's body: the text after its `---` front matter.
pub fn skill_body(skill: &str) -> &str {
    skill
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map_or(skill, |(_, body)| body)
        .trim_start()
}

/// `prompts/list`.
pub fn list() -> Value {
    json!({"prompts": [{
        "name": DATASHEET_PART,
        "title": "Create a component from datasheets",
        "description": "Author one component (pins and functions, symbol, package, footprint, binding, 3D model) from its manufacturer datasheets as drafts, with every value cited and checked by library_review_check, for a person to review and publish.",
        "arguments": [
            {"name": "part", "description": "The part name the drafts take, e.g. its manufacturer part number.", "required": true},
            {"name": "datasheets", "description": "The datasheets: file paths or URLs, one per line. You read them; Pcbcraft does not.", "required": true},
            {"name": "notes", "description": "Anything else: which variant or package, what to include or leave out.", "required": false},
        ],
    }]})
}

/// `prompts/get`: the workflow, then the task built from the arguments.
pub fn get(params: &Value) -> Result<Value, String> {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    if name != DATASHEET_PART {
        return Err(format!("unknown prompt {name:?}; prompts/list names them"));
    }
    let arguments = &params["arguments"];
    let argument = |key: &str| -> Result<String, String> {
        let text = match arguments.get(key) {
            None | Some(Value::Null) => "",
            Some(Value::String(text)) => text.trim(),
            Some(_) => return Err(format!("argument {key} must be text")),
        };
        if text.len() > MAX_ARGUMENT {
            return Err(format!("argument {key} is over {MAX_ARGUMENT} bytes"));
        }
        Ok(text.to_owned())
    };
    let part = argument("part")?;
    let datasheets = argument("datasheets")?;
    let notes = argument("notes")?;
    if part.is_empty() || datasheets.is_empty() {
        return Err("give part and datasheets".into());
    }
    let mut task = format!("## Task\n\nCreate the part \"{part}\" from these datasheets:\n\n");
    for line in datasheets.lines().map(str::trim).filter(|l| !l.is_empty()) {
        task.push_str(&format!("- {line}\n"));
    }
    if !notes.is_empty() {
        task.push_str(&format!("\nNotes: {notes}\n"));
    }
    let text = format!("{}\n{task}", skill_body(DATASHEET_PART_SKILL));
    Ok(json!({
        "description": format!("Create {part} from its datasheets"),
        "messages": [{"role": "user", "content": {"type": "text", "text": text}}],
    }))
}
