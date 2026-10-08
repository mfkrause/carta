//! Generate semicolon-terminated entity names from the vendored WHATWG data.
//!
//! Keep the crate-local data identical to the readers crate so both build independently.

#![allow(clippy::expect_used)]

use std::collections::BTreeSet;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

const ENTITIES_JSON: &str = "data/entities.json";

fn main() {
    println!("cargo:rerun-if-changed={ENTITIES_JSON}");

    let source = fs::read_to_string(ENTITIES_JSON).expect("read entities.json");
    let table: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_str(&source).expect("parse entities.json");

    let names: BTreeSet<String> = table
        .into_keys()
        .filter_map(|name| {
            name.strip_prefix('&')
                .and_then(|rest| rest.strip_suffix(';'))
                .map(str::to_owned)
        })
        .collect();

    let mut generated = String::from("pub(crate) static ENTITY_NAMES: &[&str] = &[\n");
    for name in &names {
        let _ = writeln!(generated, "    {name:?},");
    }
    generated.push_str("];\n");

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR");
    fs::write(Path::new(&out_dir).join("entity_names.rs"), generated)
        .expect("write entity_names.rs");
}
