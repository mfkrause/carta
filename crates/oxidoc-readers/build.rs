//! Generates a sorted named-character-reference lookup table from the vendored WHATWG
//! `entities.json` (see the data directory's README). Only the semicolon-terminated names are
//! recognized, keyed here without the leading `&` or trailing `;`.

// A failed build script should abort the build loudly; panicking is the intended behavior here.
#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=data/entities.json");

    let source = fs::read_to_string("data/entities.json").expect("read data/entities.json");
    let table: BTreeMap<String, Entity> =
        serde_json::from_str(&source).expect("parse entities.json");

    let mut entries: Vec<(String, String)> = table
        .into_iter()
        .filter_map(|(name, entity)| {
            let inner = name.strip_prefix('&')?.strip_suffix(';')?;
            Some((inner.to_owned(), entity.characters))
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut generated = String::from("pub(crate) static ENTITIES: &[(&str, &str)] = &[\n");
    for (name, characters) in &entries {
        let _ = writeln!(generated, "    ({name:?}, {characters:?}),");
    }
    generated.push_str("];\n");

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR");
    fs::write(Path::new(&out_dir).join("entities_table.rs"), generated)
        .expect("write entities_table.rs");
}

#[derive(serde::Deserialize)]
struct Entity {
    characters: String,
}
