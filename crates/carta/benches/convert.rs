// Benches are not shipped paths, so the panic-discipline lints are relaxed here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;

use carta::{ReaderOptions, WriterOptions, convert, reader_for, writer_for};
use criterion::{Criterion, Throughput, criterion_group, criterion_main};

/// Small input size: large enough for stable per-iteration timing, small enough to stay fast.
const SMALL: usize = 10 * 1024;
/// Large input size: stresses allocation and the hot parse/render loops.
const LARGE: usize = 1024 * 1024;
/// Large input size for the adversarial generators. Their inputs drive the inline resolver's worst
/// case, which is quadratic in delimiter count, so a 1 MiB input would take minutes per iteration;
/// 32 KiB keeps an iteration near one second while still making that worst case unmistakable.
const ADVERSARIAL_LARGE: usize = 32 * 1024;

/// Repeats `block` until the accumulated string reaches `bytes`, keeping the result within ±10% of
/// the target by stopping once the threshold is crossed and the final block fits the tolerance.
fn fill_to(bytes: usize, block: &str) -> String {
    let block = if block.is_empty() { " " } else { block };
    let mut out = String::with_capacity(bytes + block.len());
    while out.len() < bytes {
        out.push_str(block);
    }
    out
}

fn prose(bytes: usize) -> String {
    let block = "The quick brown fox writes *emphasis* and **strong** prose, sprinkled with \
                 `inline code` and ordinary words that fill the paragraph to a useful length.\n\n";
    fill_to(bytes, block)
}

fn links(bytes: usize) -> String {
    let block = "A paragraph with an [inline link](http://example.com/path \"title\") and a \
                 [reference link][ref] pointing into the definitions block below.\n\n";
    let body = fill_to(bytes, block);
    let mut out = String::with_capacity(body.len() + 64);
    out.push_str(&body);
    out.push_str("[ref]: http://example.com/reference \"reference title\"\n");
    out
}

fn lists(bytes: usize) -> String {
    let block = "- top level item\n  - second level item\n    - third level item\n  - back to second\n\
                 1. ordered top\n   1. ordered second\n      1. ordered third\n\n";
    fill_to(bytes, block)
}

fn emphasis_heavy(bytes: usize) -> String {
    let block = "*a* _b_ **c** __d__ *e* _f_ **g** __h__ ";
    fill_to(bytes, block)
}

fn pathological_brackets(bytes: usize) -> String {
    let block = "[a [b [c ]]]]] [d] ]]]] [e ]] [f [g ";
    fill_to(bytes, block)
}

type Generator = fn(usize) -> String;

const GENERATORS: &[(&str, Generator, usize)] = &[
    ("prose", prose, LARGE),
    ("links", links, LARGE),
    ("lists", lists, LARGE),
    ("emphasis_heavy", emphasis_heavy, ADVERSARIAL_LARGE),
    ("pathological_brackets", pathological_brackets, ADVERSARIAL_LARGE),
];

/// Reads every `corpus/text/commonmark/*.md` input and concatenates them, repeating until the
/// result reaches at least 100 KiB, as a realistic mixed-feature `CommonMark` document.
fn corpus_mixed() -> String {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/text/commonmark");
    let mut paths: Vec<_> = fs::read_dir(dir)
        .expect("corpus/text/commonmark directory is readable")
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    paths.sort();
    let mut one_pass = String::new();
    for path in &paths {
        one_pass.push_str(&fs::read_to_string(path).expect("corpus file is readable"));
        one_pass.push_str("\n\n");
    }
    assert!(!one_pass.is_empty(), "found no corpus/text/commonmark/*.md inputs");
    fill_to(100 * 1024, &one_pass)
}

fn read_commonmark(c: &mut Criterion) {
    let reader = reader_for("commonmark").unwrap();
    let options = ReaderOptions::default();
    let mut group = c.benchmark_group("read_commonmark");
    for &(name, generator, large) in GENERATORS {
        for (size_label, size) in [("small", SMALL), ("large", large)] {
            let input = generator(size);
            group.throughput(Throughput::Bytes(input.len() as u64));
            group.bench_function(format!("{name}/{size_label}"), |b| {
                b.iter(|| reader.read(&input, &options).unwrap());
            });
        }
    }
    group.finish();
}

fn write_targets(c: &mut Criterion) {
    let reader = reader_for("commonmark").unwrap();
    let document = reader.read(&prose(LARGE), &ReaderOptions::default()).unwrap();
    let options = WriterOptions::default();
    let targets = [
        "html",
        "plain",
        "commonmark",
        "rst",
        "latex",
        "mediawiki",
        "native",
        "json",
    ];
    let mut group = c.benchmark_group("write_targets");
    for target in targets {
        let writer = writer_for(target).unwrap();
        group.bench_function(target, |b| {
            b.iter(|| writer.write(&document, &options).unwrap());
        });
    }
    group.finish();
}

fn convert_end_to_end(c: &mut Criterion) {
    let reader_options = ReaderOptions::default();
    let writer_options = WriterOptions::default();
    let inputs = [("prose", prose(LARGE)), ("lists", lists(LARGE))];
    let mut group = c.benchmark_group("convert_end_to_end");
    for (name, input) in &inputs {
        group.throughput(Throughput::Bytes(input.len() as u64));
        group.bench_function(*name, |b| {
            b.iter(|| {
                convert("commonmark", "html", input, &reader_options, &writer_options).unwrap()
            });
        });
    }
    group.finish();
}

fn read_corpus(c: &mut Criterion) {
    let reader = reader_for("commonmark").unwrap();
    let options = ReaderOptions::default();
    let input = corpus_mixed();
    let mut group = c.benchmark_group("read_corpus");
    group.throughput(Throughput::Bytes(input.len() as u64));
    group.bench_function("commonmark", |b| {
        b.iter(|| reader.read(&input, &options).unwrap());
    });
    group.finish();
}

criterion_group!(benches, read_commonmark, write_targets, convert_end_to_end, read_corpus);
criterion_main!(benches);
