//! Generates a sorted named-character-reference lookup table from the vendored WHATWG
//! `entities.json` (see the data directory's README). Only the semicolon-terminated names are
//! recognized, keyed here without the leading `&` or trailing `;`.

#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=data/entities.json");

    let source = fs::read_to_string("data/entities.json").expect("read data/entities.json");
    let table: BTreeMap<String, serde_json::Value> =
        serde_json::from_str(&source).expect("parse entities.json");

    let mut legacy = String::from(
        "#[cfg(feature = \"asciidoc\")]\npub(crate) static LEGACY_ENTITIES: &[(&str, &str)] = &[\n",
    );
    for (name, entity) in &table {
        if let Some(name) = name.strip_prefix('&').filter(|name| !name.ends_with(';')) {
            let characters = entity
                .get("characters")
                .and_then(serde_json::Value::as_str)
                .expect("entity characters");
            let _ = writeln!(legacy, "    ({name:?}, {characters:?}),");
        }
    }
    legacy.push_str("];\n");
    let mut entries: Vec<(String, String)> = table
        .into_iter()
        .filter_map(|(name, entity)| {
            let inner = name.strip_prefix('&')?.strip_suffix(';')?;
            let characters = entity
                .get("characters")
                .and_then(serde_json::Value::as_str)
                .expect("entity characters")
                .to_owned();
            Some((inner.to_owned(), characters))
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut generated = String::from("pub(crate) static ENTITIES: &[(&str, &str)] = &[\n");
    for (name, characters) in &entries {
        let _ = writeln!(generated, "    ({name:?}, {characters:?}),");
    }
    generated.push_str("];\n");
    generated.push_str(&legacy);

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR");
    fs::write(Path::new(&out_dir).join("entities_table.rs"), generated)
        .expect("write entities_table.rs");
}
