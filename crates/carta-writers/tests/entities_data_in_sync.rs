//! Check that writer and reader entity data remain identical within the workspace.

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
