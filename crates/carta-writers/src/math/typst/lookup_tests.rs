use super::{GREEK_TYPST, GREEK_TYPST_MAP, SYMBOL_TYPST, SYMBOL_TYPST_MAP};

fn linear_find<'a>(table: &[(&'a str, &'a str)], name: &str) -> Option<&'a str> {
    table.iter().find(|(n, _)| *n == name).map(|(_, t)| *t)
}

#[test]
fn symbol_map_matches_linear_find() {
    for (name, _) in SYMBOL_TYPST {
        assert_eq!(
            SYMBOL_TYPST_MAP.get(name).copied(),
            linear_find(SYMBOL_TYPST, name)
        );
    }
}

#[test]
fn greek_map_matches_linear_find() {
    for (name, _) in GREEK_TYPST {
        assert_eq!(
            GREEK_TYPST_MAP.get(name).copied(),
            linear_find(GREEK_TYPST, name)
        );
    }
}
