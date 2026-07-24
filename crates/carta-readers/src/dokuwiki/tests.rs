use super::*;

/// Reads with the default option set and reports only whether the read completed without error,
/// so a pathological input can be checked for graceful, bounded-time handling.
fn reads_ok(input: &str) -> bool {
    DokuwikiReader
        .read(input, &ReaderOptions::default())
        .is_ok()
}

#[test]
fn adversarial_footnotes_under_open_emphasis_do_not_stall() {
    // Overlapping footnotes and unclosed `//` openers would re-parse regions super-linearly;
    // the inline backtracking budget bounds it.
    let input = format!("(({}))", "//((x)) ".repeat(400));
    assert!(reads_ok(&input));
}

#[test]
fn adversarially_nested_footnotes_do_not_stall() {
    let input = format!("{}x{}", "((".repeat(2_000), "))".repeat(2_000));
    assert!(reads_ok(&input));
}

#[test]
fn a_delimiter_dense_run_does_not_blow_up() {
    // Unclosed `//` openers with whitespace-led closers would re-scan from every position
    // (quadratic); the backtracking budget keeps it linear.
    let input = "//a ".repeat(4_000);
    assert!(reads_ok(&input));
}
