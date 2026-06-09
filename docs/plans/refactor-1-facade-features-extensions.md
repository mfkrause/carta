# Refactor 1 — Facade crate, compile-time format selection, extensions plumbing

Status: **landed**. Owner: refactor-1.
Read `../PORTING.md` §3 and `../../AGENTS.md` first. This document is self-contained: it should be
possible to execute the refactor from it alone. It does **not** add any reader or writer; it
reshapes the workspace so the upcoming format fleet (PORTING §6 Tier A/B) can land in isolated,
independently-selectable, independently-testable units, and so carta ships as a library as well as
a binary.

**Outcome.** A new `carta` library crate becomes the single public entry point (high-level
`convert()` plus low-level re-exports); every reader and writer is selectable at compile time via
per-direction Cargo features; a hand-rolled `Extension`/`Extensions` type is threaded through the
options structs; the no-`HashMap` determinism rule is lint-enforced; and a `fuzz/` crate scaffolds
the per-reader fuzzing convention. All existing tests stay green and all quality gates stay clean.

This refactor implements gaps **#1, #2, #4, #5, #6** from the post-slice-1 structure review.
Deferred (not in scope): **#3** (testkit format-genericization + golden-snapshot endgame) and **#7**
(`cargo doc` CI gate, doctests, coverage floor, the `markdown`≠CommonMark alias correction).

## 0. Goal & done criteria

**Definition of done:**

1. A new `carta` **library** crate exists and is the only crate a downstream consumer needs to
   depend on. It exposes `convert(from, to, input, &ReaderOptions, &WriterOptions) -> Result<String>`,
   `supported_input_formats()` / `supported_output_formats()`, and re-exports the AST and the
   `Reader`/`Writer` traits + concrete reader/writer types.
2. `carta-cli` depends only on `carta` (+ clap); the format-dispatch logic no longer lives in the
   binary. The `carta` binary's observable behavior is unchanged (all `crates/carta-cli/tests/cli.rs`
   pass verbatim, including the `markdown`/`html5` aliases and every error message).
3. Each reader and writer is behind a per-direction Cargo feature. `default` builds all implemented
   formats; `cargo build -p carta --no-default-features --features read-commonmark,write-html`
   builds a binary/library with exactly those two formats and nothing else compiled in. A build with
   **zero** format features also compiles (every `convert` then returns `FormatNotEnabled`).
4. A format name that is recognized but not compiled in yields `Error::FormatNotEnabled`; an
   unknown name yields `Error::UnsupportedFormat`. The two are distinguishable.
5. `carta-core` defines `Extension` (typed enum) and `Extensions` (hand-rolled fixed-word bitset,
   no dependency, no 128-variant cap, const-constructible presets). Both `ReaderOptions` and
   `WriterOptions` carry an `extensions: Extensions` field. The CommonMark reader documents that it
   implements the strict-CommonMark (empty) preset; engine generalization is explicitly deferred.
6. `clippy.toml` disallows `std::collections::HashMap` / `HashSet` workspace-wide (determinism gate).
7. A top-level `fuzz/` crate (excluded from the workspace) holds a `commonmark` libFuzzer target over
   arbitrary bytes; the per-reader fuzzing convention is documented.
8. CI gains: a `--no-default-features` build job (feature-gating bit-rot guard) and a non-blocking
   nightly fuzz smoke job.
9. `cargo nextest run --workspace --all-features`, `cargo test --doc --workspace --all-features`,
   `cargo clippy --all-targets --all-features`, and `cargo fmt --all --check` are all clean.

**Explicitly out of scope:** any new reader/writer; the configurable markdown engine (deferred until
the 2nd markdown variant lands); testkit format-genericization and committed golden snapshots (#3);
`cargo doc -D warnings`, doctests, coverage floor, and the `markdown`→CommonMark alias correction
(#7); auto-registration via `linkme`/`inventory` (rejected — keeps unsafe at zero).

## 1. Decisions locked (from the refactor-1 grilling)

| Decision | Choice |
| --- | --- |
| Facade layout | Separate `carta` **library** crate; `carta-cli` stays a thin binary depending on it |
| Facade API | High-level `convert()` + `supported_*_formats()`, **plus** re-exports of the AST and `Reader`/`Writer` traits + concrete types |
| Dispatch | Static `#[cfg]`-gated `match` in the facade; no auto-registration, no unsafe |
| Feature scheme | Per-direction features at the crate level (`carta-readers/commonmark`, `carta-writers/html`); facade exposes `read-*`/`write-*` that forward |
| Default features | `default = all implemented formats` (+ a `full` alias); minimal builds via `--no-default-features` |
| Gating guard | A `--no-default-features` CI build job |
| Options model | One shared `ReaderOptions`/`WriterOptions` (trait signatures unchanged) carrying an `extensions` field |
| Extensions type | Typed `Extension` enum + **hand-rolled fixed-word (`[u64; N]`) bitset**, in `carta-core` |
| Markdown prep | Plumb extensions through options; CommonMark reader asserts the empty preset; **defer** the engine extraction and any `markdown/` relocation |
| Determinism lint | Global `disallowed-types` for `HashMap`/`HashSet` + per-site `#[allow]` where needed (none needed today) |
| Fuzzing | `fuzz/` crate now (excluded from workspace) + one `commonmark` target + non-blocking nightly CI smoke |
| Unknown vs disabled | `UnsupportedFormat` for unknown names; new `FormatNotEnabled` for recognized-but-not-compiled-in |

## 2. Target crate layout

```
crates/
  carta-ast/        (unchanged)
  carta-core/       + extensions module; options gain `extensions`; + FormatNotEnabled error
  carta-readers/    + per-format features; optional unicode/caseless deps; cfg-gated modules
  carta-writers/    + per-format features; optional unicode dep; cfg-gated modules
  carta/            NEW facade library: registry + convert() + re-exports
  carta-cli/        depends only on `carta` (+ clap); dispatch removed
  carta-testkit/    unchanged (depends on readers/writers directly; genericization is #3, deferred)
fuzz/                NEW, excluded from the workspace (own [workspace] table)
  Cargo.toml
  fuzz_targets/commonmark.rs
```

Dependency direction is unchanged and acyclic: `ast → core → {readers, writers} → carta → carta-cli`.
The facade is the only crate that knows the set of formats; readers/writers remain mutually unaware.

## 3. `carta-core` — extensions + options + error (#5, part of #1)

### 3.1 `extensions` module

A macro generates the enum plus its `ALL`/`COUNT`/`name()` metadata so the variant list has a single
source of truth and cannot drift from the bitset sizing:

```rust
macro_rules! define_extensions {
    ($($variant:ident => $name:literal),+ $(,)?) => {
        /// A single pandoc-style format extension. Names match pandoc's documented `--from`/`--to`
        /// extension identifiers (a documented, observable contract — never derived from source).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        pub enum Extension { $($variant),+ }

        impl Extension {
            /// Every extension, in declaration order. The index of a variant here is its bit
            /// position in [`Extensions`].
            pub const ALL: &'static [Extension] = &[$(Extension::$variant),+];
            /// The number of distinct extensions.
            pub const COUNT: usize = Self::ALL.len();
            /// The extension's pandoc identifier (e.g. `"footnotes"`).
            #[must_use]
            pub const fn name(self) -> &'static str {
                match self { $(Extension::$variant => $name),+ }
            }
        }
    };
}
```

Seed it with a small, real starter set (enough to exercise the type and write meaningful presets;
not an attempt to enumerate all ~100 — that grows per format). Suggested seed, all drawn from
pandoc's documented extension names:

```rust
define_extensions! {
    Smart            => "smart",
    Strikeout        => "strikeout",
    Superscript      => "superscript",
    Subscript        => "subscript",
    PipeTables       => "pipe_tables",
    Footnotes        => "footnotes",
    TaskLists        => "task_lists",
    Autolink         => "autolink_bare_uris",
    TexMathDollars   => "tex_math_dollars",
    FencedDivs       => "fenced_divs",
    BracketedSpans   => "bracketed_spans",
}
```

The bitset is hand-rolled and const-friendly (no `Default` derive — array `Default` is not
guaranteed for all `N`):

```rust
const WORD_BITS: usize = u64::BITS as usize;
const WORDS: usize = Extension::COUNT.div_ceil(WORD_BITS);

/// A deterministic, allocation-free set of [`Extension`]s, backed by a fixed array of 64-bit words
/// indexed by each variant's position in [`Extension::ALL`]. No 128-variant ceiling.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Extensions([u64; WORDS]);

impl Default for Extensions {
    fn default() -> Self { Self::empty() }
}

impl Extensions {
    #[must_use]
    pub const fn empty() -> Self { Self([0; WORDS]) }

    #[must_use]
    pub const fn from_list(list: &[Extension]) -> Self {
        let mut words = [0u64; WORDS];
        let mut i = 0;
        while i < list.len() {
            let bit = list[i] as usize;
            words[bit / WORD_BITS] |= 1u64 << (bit % WORD_BITS);
            i += 1;
        }
        Self(words)
    }

    #[must_use]
    pub const fn contains(self, ext: Extension) -> bool {
        let bit = ext as usize;
        (self.0[bit / WORD_BITS] >> (bit % WORD_BITS)) & 1 == 1
    }

    pub fn insert(&mut self, ext: Extension) {
        let bit = ext as usize;
        self.0[bit / WORD_BITS] |= 1u64 << (bit % WORD_BITS);
    }

    pub fn remove(&mut self, ext: Extension) {
        let bit = ext as usize;
        self.0[bit / WORD_BITS] &= !(1u64 << (bit % WORD_BITS));
    }

    #[must_use]
    pub fn is_empty(self) -> bool { self.0.iter().all(|&w| w == 0) }

    /// The set's extensions in [`Extension::ALL`] (deterministic) order.
    pub fn iter(self) -> impl Iterator<Item = Extension> {
        Extension::ALL.iter().copied().filter(move |&e| self.contains(e))
    }
}
```

Custom `Debug` (the `missing_debug_implementations` lint requires one; the derived array form is
unreadable):

```rust
impl core::fmt::Debug for Extensions {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_set().entries(self.iter().map(Extension::name)).finish()
    }
}
```

`presets` submodule documents the per-flavor sets and is the seam the future markdown engine reads:

```rust
pub mod presets {
    use super::{Extension::*, Extensions};
    /// Strict CommonMark: no extensions.
    pub const COMMONMARK: Extensions = Extensions::empty();
    /// GitHub-Flavored Markdown (documented target for a future reader; no consumer yet).
    pub const GFM: Extensions =
        Extensions::from_list(&[Strikeout, PipeTables, TaskLists, Autolink]);
}
```

Unit tests (no oracle): round-trip `insert`/`remove`/`contains`; `from_list`/`iter` ordering equals
`ALL` order; `COMMONMARK.is_empty()`; `GFM` membership; `WORDS` covers `COUNT`; `name()` round-trips.

### 3.2 Options

```rust
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct ReaderOptions {
    /// Format extensions to enable. Strict-CommonMark readers ignore this (empty preset).
    pub extensions: Extensions,
}

#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct WriterOptions {
    pub extensions: Extensions,
}
```

`#[non_exhaustive]` + `Default` keep all existing `ReaderOptions::default()` / `WriterOptions::default()`
call sites compiling unchanged.

### 3.3 Error

```rust
#[error("unsupported format: {0}")]
UnsupportedFormat(String),
#[error("format '{0}' is recognized but not enabled in this build")]
FormatNotEnabled(String),
```

`lib.rs` re-exports: `pub use extensions::{Extension, Extensions};` (and `pub use extensions::presets;`).

### 3.4 CommonMark reader

No behavioral change. Add a doc-comment on `CommonmarkReader` stating it implements
`presets::COMMONMARK` and that honoring `options.extensions` is deferred to the markdown-engine work.
Do **not** thread `extensions` into the block/inline phases yet (that is the deferred engine work and
would only add dead code today).

## 4. Compile-time feature gating (#2)

### 4.1 `carta-readers`

Make the format-specific deps optional and gate the modules:

```toml
[dependencies]
carta-ast = { workspace = true }
carta-core = { workspace = true }
unicode-general-category = { workspace = true, optional = true }
caseless = { workspace = true, optional = true }

[features]
default = ["commonmark", "json"]
commonmark = ["dep:unicode-general-category", "dep:caseless"]
json = []
```

`lib.rs`:

```rust
#[cfg(feature = "commonmark")]
pub mod commonmark;
#[cfg(feature = "json")]
pub mod json;

#[cfg(feature = "commonmark")]
pub use commonmark::CommonmarkReader;
#[cfg(feature = "json")]
pub use json::JsonReader;
```

`build.rs` (entities table) is left as-is: it is cheap and the generated file is only `include!`d
from the `commonmark` module, so it is harmless when that feature is off.

### 4.2 `carta-writers`

```toml
[dependencies]
carta-ast = { workspace = true }
carta-core = { workspace = true }
unicode-general-category = { workspace = true, optional = true }

[features]
default = ["html", "json"]
html = ["dep:unicode-general-category"]
json = []
```

`lib.rs` mirrors the readers' cfg-gating.

### 4.3 `carta` facade

```toml
[dependencies]
carta-ast = { workspace = true }
carta-core = { workspace = true }
carta-readers = { workspace = true, default-features = false, optional = true }
carta-writers = { workspace = true, default-features = false, optional = true }

[features]
default = ["full"]
full = ["read-commonmark", "read-json", "write-html", "write-json"]
read-commonmark = ["dep:carta-readers", "carta-readers/commonmark"]
read-json       = ["dep:carta-readers", "carta-readers/json"]
write-html      = ["dep:carta-writers", "carta-writers/html"]
write-json      = ["dep:carta-writers", "carta-writers/json"]
```

Making the sub-crates `optional` + `dep:` means a zero-format build does not compile them at all.

`src/registry.rs` — static cfg-gated dispatch; sub-crate types referenced by full path inside the
cfg arms (no top-level `use`, so zero-format builds have no dangling imports):

```rust
use carta_core::{Error, Reader, Result, Writer};

const KNOWN_INPUT_FORMATS: &[&str] = &["commonmark", "markdown", "json"];
const KNOWN_OUTPUT_FORMATS: &[&str] = &["html", "html5", "json"];

pub fn reader_for(name: &str) -> Result<Box<dyn Reader>> {
    match name {
        #[cfg(feature = "read-json")]
        "json" => Ok(Box::new(carta_readers::JsonReader)),
        #[cfg(feature = "read-commonmark")]
        "commonmark" | "markdown" => Ok(Box::new(carta_readers::CommonmarkReader)),
        other => Err(resolution_error(other, KNOWN_INPUT_FORMATS)),
    }
}

pub fn writer_for(name: &str) -> Result<Box<dyn Writer>> {
    match name {
        #[cfg(feature = "write-json")]
        "json" => Ok(Box::new(carta_writers::JsonWriter)),
        #[cfg(feature = "write-html")]
        "html" | "html5" => Ok(Box::new(carta_writers::HtmlWriter)),
        other => Err(resolution_error(other, KNOWN_OUTPUT_FORMATS)),
    }
}

fn resolution_error(name: &str, known: &[&str]) -> Error {
    if known.contains(&name) {
        Error::FormatNotEnabled(name.to_owned())
    } else {
        Error::UnsupportedFormat(name.to_owned())
    }
}

#[must_use]
pub fn supported_input_formats() -> Vec<&'static str> {
    #[allow(unused_mut)]
    let mut formats = Vec::new();
    #[cfg(feature = "read-commonmark")] formats.push("commonmark");
    #[cfg(feature = "read-json")]       formats.push("json");
    formats
}

#[must_use]
pub fn supported_output_formats() -> Vec<&'static str> {
    #[allow(unused_mut)]
    let mut formats = Vec::new();
    #[cfg(feature = "write-html")] formats.push("html");
    #[cfg(feature = "write-json")] formats.push("json");
    formats
}
```

The alias mapping (`markdown`→commonmark, `html5`→html) is preserved exactly as the CLI had it, so
`cli.rs` stays green. (Correcting `markdown`≠CommonMark is tracked under #7, deferred.)

`src/lib.rs`:

```rust
pub use carta_ast as ast;
pub use carta_ast::Document;
pub use carta_core::{
    Error, Extension, Extensions, Reader, ReaderOptions, Result, Writer, WriterOptions,
};

mod registry;
pub use registry::{reader_for, writer_for, supported_input_formats, supported_output_formats};

/// Convert `input` from format `from` to format `to`. The returned string carries no trailing
/// newline (the CLI appends exactly one).
pub fn convert(
    from: &str,
    to: &str,
    input: &str,
    reader_options: &ReaderOptions,
    writer_options: &WriterOptions,
) -> Result<String> {
    let reader = reader_for(from)?;
    let writer = writer_for(to)?;
    let document = reader.read(input, reader_options)?;
    writer.write(&document, writer_options)
}
```

Add a small `tests/` integration test for the facade: `convert` happy paths for the compiled-in
formats and `FormatNotEnabled` vs `UnsupportedFormat` classification (needs no oracle).

### 4.4 `carta-cli`

```toml
[dependencies]
carta = { workspace = true }
clap  = { workspace = true }
```

`main.rs` drops `InputFormat`/`OutputFormat`/`FromStr`/`reader()`/`writer()` and calls
`carta::convert`. The missing-flag errors keep their current wording (`"… is required"`), the
trailing-newline ownership stays in the CLI, and file/stdin I/O is unchanged.

Add `carta = { path = "crates/carta" }` to `[workspace.dependencies]`.

## 5. Determinism lint (#4)

`clippy.toml`:

```toml
disallowed-types = [
    { path = "std::collections::HashMap", reason = "non-deterministic iteration; use BTreeMap for any map reaching the AST or a writer (AGENTS.md)" },
    { path = "std::collections::HashSet", reason = "non-deterministic iteration; use BTreeSet (AGENTS.md)" },
]
```

No code uses `HashMap`/`HashSet` today (verified), so no `#[allow]` sites are needed. Keep the
existing `allow-*-in-tests` entries.

## 6. Fuzz scaffolding (#6)

`fuzz/Cargo.toml` (own `[workspace]` table detaches it from the root workspace, the cargo-fuzz
convention):

```toml
[package]
name = "carta-fuzz"
version = "0.0.0"
edition = "2024"
publish = false

[package.metadata]
cargo-fuzz = true

[dependencies]
libfuzzer-sys = "0.4"
carta-core = { path = "../crates/carta-core" }
carta-readers = { path = "../crates/carta-readers" }

[[bin]]
name = "commonmark"
path = "fuzz_targets/commonmark.rs"
test = false
doc = false
bench = false

[profile.release]
debug = 1

[workspace]
```

`fuzz/fuzz_targets/commonmark.rs`:

```rust
#![no_main]
use libfuzzer_sys::fuzz_target;
use carta_core::{Reader, ReaderOptions};
use carta_readers::CommonmarkReader;

// Bar: no panic, no hang on any input (PORTING §8). UTF-8 only — the reader's contract is `&str`.
fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = CommonmarkReader.read(text, &ReaderOptions::default());
    }
});
```

Add a short `fuzz/README.md` documenting the convention (one target per reader, `cargo +nightly fuzz
run <reader>`), and a `.gitignore` for `fuzz/corpus/` and `fuzz/artifacts/`.

## 7. CI changes (parts of #2 and #6)

Add to `.github/workflows/ci.yml`:

- A **`minimal`** job (stable toolchain, no oracle needed): builds the feature-gating guard.
  ```
  cargo build -p carta --no-default-features --features read-commonmark,write-html
  cargo build -p carta --no-default-features            # zero formats must still compile
  ```
- A **`fuzz-smoke`** job, `continue-on-error: true` (non-blocking), nightly toolchain, installs
  `cargo-fuzz`, runs `cargo +nightly fuzz run commonmark -- -max_total_time=30`.

The existing `check` job keeps `--all-features` for clippy/test/doctest (now exercises every format
feature). No change to the oracle-provisioning steps.

## 8. Work breakdown (suggested commit sequence, Conventional Commits)

Each step ends green (`fmt` + `clippy --all-targets --all-features` + `nextest --workspace`).

1. `feat(core): add Extension/Extensions set, presets, and options field` — §3.1–3.3 (incl. the
   `FormatNotEnabled` variant) + unit tests; CommonMark reader doc note (§3.4).
2. `refactor(readers): gate formats behind per-format features` — §4.1.
3. `refactor(writers): gate formats behind per-format features` — §4.2.
4. `feat(carta): add facade library with registry and convert()` — §4.3 + facade tests + workspace
   dep entry.
5. `refactor(cli): dispatch through the carta facade` — §4.4.
6. `style(lint): disallow HashMap/HashSet for deterministic output` — §5.
7. `test(fuzz): scaffold cargo-fuzz crate with a commonmark target` — §6.
8. `ci: add no-default-features build and nightly fuzz smoke jobs` — §7.
9. `docs(porting): record the facade + feature-gating + extensions layout` — update PORTING §3 and
   the Build & test section of `AGENTS.md`; flip this plan's status to **landed**.

## 9. Risks / watch-items

- **`dep:` + `optional` ordering.** A zero-format facade build must compile with no dangling
  references — keep all sub-crate paths inside cfg arms, never in top-level `use`. The `minimal` CI
  job's zero-format build is the guard.
- **Const-fn bitset on edition 2024 / Rust 1.93.** `from_list`/`contains` use array indexing inside
  `const fn`; this is supported on the pinned toolchain. If a `const`-eval issue surfaces, fall back
  to non-`const` `from_list` (presets become `LazyLock`), but prefer const.
- **`indexing_slicing` lint vs. the bitset.** The restriction lint warns on `xs[i]`. The bitset's
  word indexing is provably in-bounds (`bit / WORD_BITS < WORDS`), but to satisfy the lint without
  `#[allow]` clutter, prefer `.get()/.get_mut()` with a `?`/fallback in the non-const methods; the
  `const fn` paths may need a localized `#[allow(clippy::indexing_slicing)]` with a `// SAFETY-style`
  justification comment (`.get()` is not yet stable in all const contexts). Keep such allows to the
  two const methods only.
- **`--all-features` interaction with `full`.** `full` is a pure aggregate of the `read-*`/`write-*`
  features; `--all-features` enabling both `full` and its parts is idempotent. No diamond issues.
- **cargo-fuzz needs nightly + a sanitizer**; that is why the smoke job is separate and
  non-blocking, and why `fuzz/` is detached from the stable-pinned workspace.
- **Behavior parity for the CLI.** The aliases and exact error substrings are pinned by `cli.rs`;
  preserve them. Do not "fix" `markdown`→CommonMark here (tracked under #7).
