//! Layer 1 end-to-end golden tests: snapshot carta's full-pipeline output for each `corpus/text/<fmt>/*`
//! case rendered across a bounded set of targets, composing a real reader with a real writer offline.
//!
//! The reader and writer golden suites split the pipeline at JSON: `golden_reader.rs` freezes
//! `text -> json` and `golden_writer.rs` freezes `json -> target`. Neither exercises a reader composed
//! with a non-JSON writer, so a defect that only surfaces when a specific reader's AST shape meets a
//! specific writer stays invisible. This suite closes that gap: it freezes `text -> target` for every
//! text corpus case across the target set below. Snapshots run fully offline and are reviewed with
//! `cargo insta review`; never hand-edit the `.snap` files.
//!
//! Each reader format gets its own `#[test]` so a single failing case cannot abort the rest and nextest
//! can run and parallelize them independently. A guard test asserts the macro's format list still
//! equals the `corpus/text/` directory set, so a new corpus directory without a matching test fails
//! loudly. Each `(format, target)` pair is snapshotted only when both sides are compiled into the
//! build; a pair whose reader or writer is absent is skipped at runtime.

// Integration-test harness code: panicking on a known corpus case is the idiomatic assertion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use carta::{ReaderOptions, WriterOptions};
use common::{corpus_cases, corpus_groups};

/// The bounded target set every text case is rendered to. JSON is omitted because `golden_reader.rs`
/// already freezes `text -> json`; the remaining targets each exercise a distinct writer against every
/// reader's AST.
const TARGETS: &[&str] = &[
    "html",
    "latex",
    "rst",
    "plain",
    "commonmark",
    "mediawiki",
    "native",
];

/// `(format, target, label)` triples whose composition cannot yet be rendered. Each entry is a latent
/// composition defect: the reader produces an AST shape the target's writer mishandles. Fixing the
/// writer removes the entry and adds the newly frozen snapshot.
const SKIP: &[(&str, &str, &str)] = &[];

/// Render every `corpus/text/<fmt>/*` case to each compiled target in [`TARGETS`], when the format's
/// reader is compiled in. A format whose reader is absent from this build is skipped at runtime, as is
/// any target whose writer is absent.
fn e2e_snapshots_for(fmt: &str) {
    if !carta::supported_input_formats().contains(&fmt) {
        return;
    }
    let writers = carta::supported_output_formats();
    let cases: Vec<_> = corpus_cases("text")
        .into_iter()
        .filter(|case| case.group == fmt)
        .collect();
    for &target in TARGETS {
        if !writers.contains(&target) {
            continue;
        }
        for case in &cases {
            if SKIP
                .iter()
                .any(|(f, t, l)| *f == fmt && *t == target && *l == case.label)
            {
                continue;
            }
            let output = carta::convert_text(
                fmt,
                target,
                &case.input,
                &ReaderOptions::default(),
                &WriterOptions::default(),
            )
            .unwrap_or_else(|error| panic!("convert {fmt}/{} -> {target}: {error}", case.label));
            insta::assert_snapshot!(format!("{fmt}__{target}__{}", case.label), output);
        }
    }
}

/// Assert the macro's covered-format list equals the `corpus/text/` directory set, so a new corpus
/// directory without a matching test entry fails loudly.
fn assert_formats_partitioned(covered: &[&str]) {
    let mut expected = corpus_groups("text");
    expected.sort();
    let mut actual: Vec<String> = covered.iter().map(|fmt| (*fmt).to_owned()).collect();
    actual.sort();
    assert_eq!(
        actual, expected,
        "corpus/text directories and the e2e macro's test entries have diverged"
    );
}

macro_rules! e2e_golden {
    ($helper:ident, $list:ident; $($name:ident => $fmt:literal),+ $(,)?) => {
        $(
            #[test]
            fn $name() { $helper($fmt); }
        )+
        const $list: &[&str] = &[$($fmt),+];
    };
}

e2e_golden! {
    e2e_snapshots_for, E2E_TEXT_FORMATS;
    e2e_snapshots_commonmark => "commonmark",
    e2e_snapshots_csv => "csv",
    e2e_snapshots_dokuwiki => "dokuwiki",
    e2e_snapshots_html => "html",
    e2e_snapshots_ipynb => "ipynb",
    e2e_snapshots_jira => "jira",
    e2e_snapshots_json => "json",
    e2e_snapshots_latex => "latex",
    e2e_snapshots_man => "man",
    e2e_snapshots_mediawiki => "mediawiki",
    e2e_snapshots_native => "native",
    e2e_snapshots_opml => "opml",
    e2e_snapshots_org => "org",
    e2e_snapshots_rst => "rst",
    e2e_snapshots_rtf => "rtf",
    e2e_snapshots_tsv => "tsv",
}

#[test]
fn e2e_snapshots_all_formats_partitioned() {
    assert_formats_partitioned(E2E_TEXT_FORMATS);
}
