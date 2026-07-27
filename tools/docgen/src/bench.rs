//! Renders the benchmark artifacts from `docs/benchmarks.toml`.
//!
//! The TOML holds measurements and nothing else; every headline figure on the page and on the site
//! is derived here, so no claim can outlive the numbers behind it.

use std::error::Error;

use serde::{Deserialize, Serialize};

use crate::render::{generated_header, wrap};

const INTRO: &str = "Measured on {runner}: carta {carta} against pandoc {pandoc}, driven by hyperfine {hyperfine} (warmup {warmup}, {runs} runs).";

const HOW_TO_READ: &str = "Both tools run with identical `-f/-t` flags; pandoc is configured so both tools produce equivalent output and do equivalent work. Times are wall-clock end-to-end (process start included). `speedup` = pandoc mean ÷ carta mean. `MB/s` is carta throughput over the actual input size. RSS is peak resident memory from a single `/usr/bin/time` run. The HTML and LaTeX targets include syntax highlighting of code blocks in both tools.";

const REPRODUCE: &str = "Reproduce with `tools/bench-suite/run.sh all`. Numbers are machine-specific; yours will differ.";

/// The whole of `docs/benchmarks.toml`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Benchmarks {
    pub meta: Meta,
    pub binary: Binary,
    #[serde(default, rename = "result")]
    pub results: Vec<Measurement>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Meta {
    pub date: String,
    pub carta: String,
    pub pandoc: String,
    pub hyperfine: String,
    pub runner: String,
    pub warmup: u32,
    pub runs: u32,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Binary {
    pub carta: u64,
    pub pandoc: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Measurement {
    pub surface: String,
    pub group: String,
    pub bytes: u64,
    pub runs: u32,
    pub carta_ms: f64,
    pub carta_sd_ms: f64,
    pub pandoc_ms: f64,
    pub pandoc_sd_ms: f64,
    #[serde(default)]
    pub carta_rss: Option<u64>,
    #[serde(default)]
    pub pandoc_rss: Option<u64>,
}

impl Measurement {
    pub fn speedup(&self) -> f64 {
        if self.carta_ms > 0.0 {
            self.pandoc_ms / self.carta_ms
        } else {
            0.0
        }
    }

    /// carta throughput in MB/s over the actual input size.
    fn throughput(&self) -> f64 {
        if self.carta_ms > 0.0 {
            (self.bytes as f64 / 1_048_576.0) / (self.carta_ms / 1000.0)
        } else {
            0.0
        }
    }

    fn memory_ratio(&self) -> Option<f64> {
        match (self.carta_rss, self.pandoc_rss) {
            (Some(carta), Some(pandoc)) if carta > 0 => Some(pandoc as f64 / carta as f64),
            _ => None,
        }
    }
}

/// The figures the summary quotes, each one a fact about the measurements below it.
#[derive(Serialize)]
pub struct Headline {
    #[serde(rename = "endToEndSpeedup")]
    end_to_end: Range,
    #[serde(rename = "surfaceSpeedup")]
    surface: f64,
    #[serde(rename = "binarySize")]
    binary_size: BinarySize,
    #[serde(rename = "memoryRatio")]
    memory: Range,
}

#[derive(Serialize)]
struct Range {
    low: f64,
    high: f64,
}

#[derive(Serialize)]
struct BinarySize {
    carta: String,
    pandoc: String,
    ratio: f64,
}

impl Headline {
    fn of(benchmarks: &Benchmarks) -> Self {
        let end_to_end = Range::over(
            benchmarks
                .results
                .iter()
                .filter(|result| result.surface == "e2e")
                .map(Measurement::speedup),
        );
        let surface = Range::over(
            benchmarks
                .results
                .iter()
                .filter(|result| result.surface == "reader" || result.surface == "writer")
                .map(Measurement::speedup),
        );
        Self {
            end_to_end,
            surface: surface.high,
            binary_size: BinarySize {
                carta: bytes(Some(benchmarks.binary.carta)),
                pandoc: bytes(Some(benchmarks.binary.pandoc)),
                ratio: ratio(benchmarks.binary.pandoc, benchmarks.binary.carta),
            },
            memory: Range::over(
                benchmarks
                    .results
                    .iter()
                    .filter_map(Measurement::memory_ratio),
            ),
        }
    }
}

impl Range {
    fn over(values: impl Iterator<Item = f64>) -> Self {
        let mut low = f64::MAX;
        let mut high: f64 = 0.0;
        for value in values {
            low = low.min(value);
            high = high.max(value);
        }
        Self {
            low: round(if low == f64::MAX { 0.0 } else { low }, 1),
            high: round(high, 1),
        }
    }

    /// The range as it reads in prose, collapsed to one figure when the ends round together.
    fn prose(&self) -> String {
        let (low, high) = (self.low.round(), self.high.round());
        if low == high {
            format!("~{low:.0}×")
        } else {
            format!("~{low:.0}–{high:.0}×")
        }
    }
}

/// The full text of `docs/BENCHMARKS.md`.
pub fn markdown(benchmarks: &Benchmarks) -> String {
    let headline = Headline::of(benchmarks);
    let mut out = generated_header("docs/benchmarks.toml");
    out.push_str("\n\n# Benchmarks: carta vs pandoc\n\n");
    out.push_str(&wrap(&intro(&benchmarks.meta)));
    out.push_str("\n\n## Headline\n\n");
    out.push_str(&wrap(&summary(&headline)));
    out.push_str("\n\n## How to read this\n\n");
    out.push_str(&wrap(HOW_TO_READ));
    out.push_str("\n\n");
    out.push_str(&wrap(REPRODUCE));
    out.push('\n');

    let mut current: Option<(&str, &str)> = None;
    for result in &benchmarks.results {
        let heading = (result.surface.as_str(), result.group.as_str());
        if current != Some(heading) {
            out.push_str(&format!("\n## {}: {}\n\n", heading.0, heading.1));
            out.push_str("| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |\n");
            out.push_str("|--------|---------------------|---------------------|---------|------------|-----------|------------|\n");
            current = Some(heading);
        }
        out.push_str(&row(result));
    }

    out.push_str("\n## binary size\n\n");
    out.push_str("| binary | size       | ratio |\n|--------|------------|-------|\n");
    out.push_str(&format!(
        "| {:<6} | {:>10} | {:>5} |\n",
        "carta",
        bytes(Some(benchmarks.binary.carta)),
        "1.0x"
    ));
    out.push_str(&format!(
        "| {:<6} | {:>10} | {:>5} |\n",
        "pandoc",
        bytes(Some(benchmarks.binary.pandoc)),
        format!("{:.0}x", headline.binary_size.ratio)
    ));
    out
}

/// `benchmarks.json`: the same measurements plus the derived headline, for the site to present.
pub fn json(benchmarks: &Benchmarks) -> Result<String, Box<dyn Error>> {
    #[derive(Serialize)]
    struct Case<'a> {
        surface: &'a str,
        group: &'a str,
        size: String,
        bytes: u64,
        runs: u32,
        #[serde(rename = "cartaMs")]
        carta_ms: f64,
        #[serde(rename = "pandocMs")]
        pandoc_ms: f64,
        speedup: f64,
        #[serde(rename = "cartaRss")]
        carta_rss: Option<u64>,
        #[serde(rename = "pandocRss")]
        pandoc_rss: Option<u64>,
    }

    #[derive(Serialize)]
    struct File<'a> {
        provenance: &'a Meta,
        headline: Headline,
        binary: &'a Binary,
        results: Vec<Case<'a>>,
    }

    let file = File {
        provenance: &benchmarks.meta,
        headline: Headline::of(benchmarks),
        binary: &benchmarks.binary,
        results: benchmarks
            .results
            .iter()
            .map(|result| Case {
                surface: &result.surface,
                group: &result.group,
                size: size(result.bytes),
                bytes: result.bytes,
                runs: result.runs,
                carta_ms: result.carta_ms,
                pandoc_ms: result.pandoc_ms,
                speedup: round(result.speedup(), 1),
                carta_rss: result.carta_rss,
                pandoc_rss: result.pandoc_rss,
            })
            .collect(),
    };

    let mut json = serde_json::to_string_pretty(&file)?;
    json.push('\n');
    Ok(json)
}

fn intro(meta: &Meta) -> String {
    INTRO
        .replace("{runner}", &meta.runner)
        .replace("{carta}", &meta.carta)
        .replace("{pandoc}", &meta.pandoc)
        .replace("{hyperfine}", &meta.hyperfine)
        .replace("{warmup}", &meta.warmup.to_string())
        .replace("{runs}", &meta.runs.to_string())
}

fn summary(headline: &Headline) -> String {
    format!(
        "carta is {} faster end-to-end across formats and sizes, and up to ~{:.0}× on individual reader/writer surfaces. Its binary is ~{:.0}× smaller ({} vs {}), and it uses {} less peak memory.",
        headline.end_to_end.prose(),
        headline.surface,
        headline.binary_size.ratio,
        headline.binary_size.carta,
        headline.binary_size.pandoc,
        headline.memory.prose(),
    )
}

fn row(result: &Measurement) -> String {
    format!(
        "| {:<6} | {:>8} ms ± {:<5} | {:>8} ms ± {:<5} | {:>7} | {:>10} | {:>9} | {:>10} |\n",
        size(result.bytes),
        format!("{:.2}", result.carta_ms),
        format!("{:.2}", result.carta_sd_ms),
        format!("{:.2}", result.pandoc_ms),
        format!("{:.2}", result.pandoc_sd_ms),
        format!("{:.1}x", result.speedup()),
        format!("{:.1}", result.throughput()),
        bytes(result.carta_rss),
        bytes(result.pandoc_rss),
    )
}

/// An input size, rounded to whole units the way the size columns read.
fn size(value: u64) -> String {
    match value {
        v if v >= 1_048_576 => format!("{:.0} MB", v as f64 / 1_048_576.0),
        v if v >= 1024 => format!("{:.0} KB", v as f64 / 1024.0),
        v => format!("{v} B"),
    }
}

/// A memory or binary size to one decimal, or `-` where the host could not measure it.
fn bytes(value: Option<u64>) -> String {
    match value {
        Some(v) if v >= 1_048_576 => format!("{:.1} MB", v as f64 / 1_048_576.0),
        Some(v) if v >= 1024 => format!("{:.1} KB", v as f64 / 1024.0),
        Some(v) => format!("{v} B"),
        None => "-".to_owned(),
    }
}

fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        round(numerator as f64 / denominator as f64, 1)
    }
}

fn round(value: f64, places: u32) -> f64 {
    let scale = 10_f64.powi(places as i32);
    (value * scale).round() / scale
}
