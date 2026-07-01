//! Generates a sorted set of valid named-character-reference names from the vendored WHATWG
//! `entities.json`. The `CommonMark` writer consults it to decide when a literal `&` must be escaped
//! so running text is not re-read as a character reference. Only the semicolon-terminated names are
//! recognized, stored without the leading `&` or trailing `;`. The data file lives inside this crate
//! so it builds in isolation; a test keeps it byte-identical to the readers crate's copy.

// A failed build script should abort the build loudly; panicking is the intended behavior here.
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
