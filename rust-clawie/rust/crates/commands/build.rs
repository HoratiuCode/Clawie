use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=commands.json");
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("slash_commands.rs");
    let mut f =
        BufWriter::new(File::create(&dest_path).expect("Failed to create slash_commands.rs"));

    // Read commands.json
    let commands_json =
        std::fs::read_to_string("commands.json").expect("Failed to read commands.json");
    let commands: serde_json::Value =
        serde_json::from_str(&commands_json).expect("Failed to parse commands.json");
    let commands_array = commands
        .as_array()
        .expect("commands.json must be a JSON array");

    // 1. Generate SLASH_COMMAND_SPECS array
    writeln!(f, "const SLASH_COMMAND_SPECS: &[SlashCommandSpec] = &[").unwrap();
    for cmd in commands_array {
        let name = cmd["name"].as_str().unwrap();

        let aliases_val = cmd["aliases"].as_array().unwrap();
        let aliases_strs: Vec<String> = aliases_val
            .iter()
            .map(|v| format!("\"{}\"", v.as_str().unwrap()))
            .collect();
        let aliases_formatted = aliases_strs.join(", ");

        let summary = cmd["summary"].as_str().unwrap();

        let argument_hint = match cmd["argument_hint"].as_str() {
            Some(hint) => format!("Some(\"{}\")", hint),
            None => "None".to_string(),
        };

        let resume_supported = cmd["resume_supported"].as_bool().unwrap();

        writeln!(
            f,
            "    SlashCommandSpec {{ name: \"{}\", aliases: &[{}], summary: \"{}\", argument_hint: {}, resume_supported: {} }},",
            name, aliases_formatted, summary, argument_hint, resume_supported
        ).unwrap();
    }
    writeln!(f, "];").unwrap();
    writeln!(f).unwrap();

    // 2. Generate slash_command_category function
    writeln!(
        f,
        "fn slash_command_category(name: &str) -> &'static str {{"
    )
    .unwrap();
    writeln!(f, "    match name {{").unwrap();

    // Group commands by category
    let mut categories_map = std::collections::BTreeMap::new();
    for cmd in commands_array {
        let name = cmd["name"].as_str().unwrap();
        let category = cmd["category"].as_str().unwrap();
        categories_map
            .entry(category.to_string())
            .or_insert_with(Vec::new)
            .push(name.to_string());
    }

    for (category, names) in categories_map {
        let names_match: Vec<String> = names.iter().map(|n| format!("\"{}\"", n)).collect();
        let names_pattern = names_match.join(" | ");
        writeln!(f, "        {} => \"{}\",", names_pattern, category).unwrap();
    }

    writeln!(f, "        _ => \"Other\",").unwrap();
    writeln!(f, "    }}").unwrap();
    writeln!(f, "}}").unwrap();
}
