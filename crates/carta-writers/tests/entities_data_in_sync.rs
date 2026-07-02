//! The writers crate vendors its own copy of the WHATWG `entities.json` so it builds in isolation.
//! This guard fails if that copy drifts from the readers crate's copy, so the two are always updated
//! together. It reads a sibling crate's file, so it only runs inside the workspace (never from a
//! published tarball, where integration tests are not compiled).

#[test]
fn writers_entities_json_matches_readers_copy() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let ours = std::fs::read(format!("{manifest_dir}/data/entities.json"))
        .expect("read writers data/entities.json");
    let theirs = std::fs::read(format!(
        "{manifest_dir}/../carta-readers/data/entities.json"
    ))
    .expect("read readers data/entities.json");
    assert!(
        ours == theirs,
        "carta-writers/data/entities.json has drifted from carta-readers/data/entities.json; \
         update both copies together"
    );
}
