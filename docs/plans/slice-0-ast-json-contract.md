# Slice 0 — AST + JSON contract

Status: **planned** (not yet implemented). Owner: slice-0.
Read `../PORTING.md` §3–5 and `../../AGENTS.md` first. This plan is self-contained: it should be
possible to implement the slice from this document alone.

## 0. Goal & done criteria

Freeze the document AST and its JSON interchange representation in `carta-ast`, wire a JSON
reader and JSON writer so `carta -f json -t json` runs end-to-end, and build a differential
round-trip harness in `carta-testkit`.

**Definition of done:**

1. `carta -f json -t json` reads pandoc-shaped JSON on stdin (or a file) and writes the
   re-serialized document to stdout (or `-o`).
2. Round-trip gate is green: for every golden JSON minted from the corpus's `.native` files,
   `Value(pandoc_json) == Value(carta_output)` (see §6). Hard-requires the fetched corpus.
3. Tag-coverage assertion passes: every AST tag in the known set (§3) appears at least once across
   the minted corpus; the test fails listing any missing tag.
4. Committed hand-authored input fixtures pass an offline self-round-trip (no oracle needed).
5. No `todo!` reachable on the json↔json path. The only sanctioned `todo!`s are: non-json
   `--from`/`--to` values, and api-version compatibility validation.
6. Clippy clean under the workspace lints (no `unwrap`/`expect`/`panic`/indexing in shipped code);
   `cargo fmt --check` clean.

**Explicitly out of scope:** the `native` text format, any human-facing reader/writer, api-version
validation, templates/standalone output, streaming I/O.

## 1. The wire contract (derived from the pinned binary, never from memory)

All shapes below were observed from `carta/.oracle/bin/pandoc` (pandoc 3.10,
`pandoc-api-version` = `[1,23,1,2]`). Re-derive with the binary if anything is unclear; never read
pandoc source.

Top-level document object (NOT `t`/`c`-tagged), keys in this exact order:

```json
{"pandoc-api-version":[1,23,1,2],"meta":{ ... },"blocks":[ ... ]}
```

- Output is **compact** (no spaces after `:`/`,`) and ends with a single trailing `\n`.
- `meta` is a JSON object: `String -> MetaValue`, keys sorted (Aeson sorts; we use `BTreeMap`).
- `blocks` is an array of Block nodes.

Node encoding (`Block`, `Inline`, and the small enums): adjacently tagged
`{"t":Tag}` for nullary variants, `{"t":Tag,"c":Content}` otherwise. Content is the variant's
single field, or a JSON array of its fields when it has more than one.

`Attr` is a 3-element array: `[id, [class, ...], [[key, val], ...]]`. Empty is `["",[],[]]`.

`Target` (link/image destination) is a 2-element array: `[url, title]`.

Confirmed example shapes (abbreviated):

```
Str            {"t":"Str","c":"hi"}
Space          {"t":"Space"}                     (also SoftBreak, LineBreak)
Emph           {"t":"Emph","c":[<inlines>]}      (also Strong, Underline, Strikeout,
                                                   Superscript, Subscript, SmallCaps)
Code           {"t":"Code","c":[<Attr>,"text"]}
Math           {"t":"Math","c":[{"t":"InlineMath"},"x"]}   (or {"t":"DisplayMath"})
RawInline      {"t":"RawInline","c":["html","<b>"]}
Quoted         {"t":"Quoted","c":[{"t":"DoubleQuote"},[<inlines>]]}  (or SingleQuote)
Link           {"t":"Link","c":[<Attr>,[<inlines>],["url","title"]]}
Image          {"t":"Image","c":[<Attr>,[<inlines>],["url","title"]]}
Note           {"t":"Note","c":[<blocks>]}
Span           {"t":"Span","c":[<Attr>,[<inlines>]]}
Cite           {"t":"Cite","c":[[<Citation>...],[<inlines>]]}

Para           {"t":"Para","c":[<inlines>]}      (also Plain)
LineBlock      {"t":"LineBlock","c":[[<inlines>],[<inlines>]...]}
CodeBlock      {"t":"CodeBlock","c":[<Attr>,"text"]}
RawBlock       {"t":"RawBlock","c":["html","<b>"]}
BlockQuote     {"t":"BlockQuote","c":[<blocks>]}
BulletList     {"t":"BulletList","c":[[<blocks>],[<blocks>]...]}
OrderedList    {"t":"OrderedList","c":[[<start:int>,<ListNumberStyle>,<ListNumberDelim>],
                                       [[<blocks>],...]]}
DefinitionList {"t":"DefinitionList","c":[[[<inlines>],[[<blocks>],...]], ...]}
Header         {"t":"Header","c":[<level:int>,<Attr>,[<inlines>]]}
HorizontalRule {"t":"HorizontalRule"}
Div            {"t":"Div","c":[<Attr>,[<blocks>]]}
Figure         {"t":"Figure","c":[<Attr>,<Caption>,[<blocks>]]}
Table          {"t":"Table","c":[<Attr>,<Caption>,[<ColSpec>...],<TableHead>,
                                  [<TableBody>...],<TableFoot>]}
```

Small enums (all `{"t":Tag}` nullary unless noted):
- `QuoteType`: `SingleQuote`, `DoubleQuote`.
- `MathType`: `InlineMath`, `DisplayMath`.
- `ListNumberStyle`: `DefaultStyle`, `Example`, `Decimal`, `LowerRoman`, `UpperRoman`,
  `LowerAlpha`, `UpperAlpha`.
- `ListNumberDelim`: `DefaultDelim`, `Period`, `OneParen`, `TwoParens`.
- `Alignment`: `AlignLeft`, `AlignRight`, `AlignCenter`, `AlignDefault`.
- `ColWidth`: `ColWidthDefault` (nullary) **or** `{"t":"ColWidth","c":<f64>}`.
- `CitationMode`: `AuthorInText`, `SuppressAuthor`, `NormalCitation`.

`Format` (in RawInline/RawBlock) is a bare JSON string (`"html"`), modeled as a `Text` newtype.

`MetaValue` (`t`/`c`-tagged):
- `MetaMap` → object `String -> MetaValue`
- `MetaList` → `[<MetaValue>...]`
- `MetaBool` → `true`/`false`
- `MetaString` → `"..."`
- `MetaInlines` → `[<inlines>]`
- `MetaBlocks` → `[<blocks>]`

`Citation` is a **named-key JSON object** (camelCase), NOT `t`/`c`-tagged:

```json
{"citationId":"key","citationPrefix":[<inlines>],"citationSuffix":[<inlines>],
 "citationMode":{"t":"NormalCitation"},"citationNoteNum":1,"citationHash":0}
```

Table sub-structures (all **arrays**, observed shape):
- `Caption` = `[<ShortCaption | null>, [<blocks>]]`; `ShortCaption` = `[<inlines>]`.
- `ColSpec` = `[<Alignment>, <ColWidth>]`.
- `TableHead` = `[<Attr>, [<Row>...]]`.
- `TableBody` = `[<Attr>, <RowHeadColumns:int>, [<Row>...], [<Row>...]]`
  (intermediate head rows, then body rows).
- `TableFoot` = `[<Attr>, [<Row>...]]`.
- `Row` = `[<Attr>, [<Cell>...]]`.
- `Cell` = `[<Attr>, <Alignment>, <RowSpan:int>, <ColSpan:int>, [<blocks>]]`.

## 2. Known AST tag set (for the coverage assertion in §6)

Blocks (14): `Plain Para LineBlock CodeBlock RawBlock BlockQuote OrderedList BulletList
DefinitionList Header HorizontalRule Table Figure Div`.

Inlines (20): `Str Emph Underline Strong Strikeout Superscript Subscript SmallCaps Quoted Cite
Code Space SoftBreak LineBreak Math RawInline Link Image Note Span`.

MetaValues (6): `MetaMap MetaList MetaBool MetaString MetaInlines MetaBlocks`.

The coverage test collects every `t` value (recursively, including inside `meta`) seen across the
minted corpus and asserts this set is fully covered. If pandoc 3.10 never emits a tag from a
`.native` corpus file (candidates: `MetaString`, `MetaBool`, some `ListNumberStyle`s), add one
targeted committed fixture (see §7) and document why. If the binary emits a tag NOT in this set,
the test must also fail (our set is stale) — assert set-equality of {known} vs {seen ∪ fixtures},
not just subset.

## 3. AST modeling (`carta-ast`)

Single module tree under `crates/carta-ast/src/`. Suggested files: `lib.rs` (re-exports + docs +
the api-version constant), `ast.rs` (types), `serde_impls.rs` (manual array (de)serializers).

### 3.1 Text and shared aliases

```rust
/// All textual payloads in the AST. Owned today; swappable to a compact-string type later
/// without touching call sites.
pub type Text = String;
```

### 3.2 The api-version constant (the one sanctioned upstream-name occurrence)

```rust
/// The JSON object key carrying the AST schema version. Opaque external protocol identifier;
/// confined to this constant per AGENTS.md "Source hygiene".
pub const API_VERSION_KEY: &str = "pandoc-api-version";

/// Default AST schema version stamped onto freshly constructed documents (slice 1+ readers).
/// Round-tripped documents echo the version they were parsed from instead (see `Document`).
pub const DEFAULT_API_VERSION: ApiVersion = ApiVersion(/* [1,23,1,2] as a const-constructible value */);
```

`ApiVersion` wraps the integer array. Use `Vec<u32>` inside (const default can be built via a
`const fn` or a `&'static [u32]` + accessor; pick whatever keeps it `const`-usable and lossless).
Round-trip stores exactly what was read.

### 3.3 Core types

```rust
pub struct Document {
    pub api_version: ApiVersion,
    pub meta: BTreeMap<Text, MetaValue>,   // sorted keys
    pub blocks: Vec<Block>,
}

pub struct Attr {
    pub id: Text,
    pub classes: Vec<Text>,
    pub attributes: Vec<(Text, Text)>,     // ordered; preserves source order
}

pub struct Target { pub url: Text, pub title: Text }

pub struct Format(pub Text);

pub enum Block { /* 14 variants, fields per §1 */ }
pub enum Inline { /* 20 variants, fields per §1 */ }
pub enum MetaValue { /* 6 variants */ }

pub enum QuoteType { SingleQuote, DoubleQuote }
pub enum MathType { InlineMath, DisplayMath }
pub enum ListNumberStyle { DefaultStyle, Example, Decimal, LowerRoman, UpperRoman, LowerAlpha, UpperAlpha }
pub enum ListNumberDelim { DefaultDelim, Period, OneParen, TwoParens }
pub enum Alignment { AlignLeft, AlignRight, AlignCenter, AlignDefault }
pub enum ColWidth { ColWidth(f64), ColWidthDefault }
pub enum CitationMode { AuthorInText, SuppressAuthor, NormalCitation }

pub struct ListAttributes { pub start: i32, pub style: ListNumberStyle, pub delim: ListNumberDelim }

pub struct Citation {
    pub id: Text,                  // citationId
    pub prefix: Vec<Inline>,       // citationPrefix
    pub suffix: Vec<Inline>,       // citationSuffix
    pub mode: CitationMode,        // citationMode
    pub note_num: i32,             // citationNoteNum
    pub hash: i32,                 // citationHash
}

pub struct Caption { pub short: Option<Vec<Inline>>, pub long: Vec<Block> }
pub struct ColSpec { pub align: Alignment, pub width: ColWidth }
pub struct TableHead { pub attr: Attr, pub rows: Vec<Row> }
pub struct TableBody { pub attr: Attr, pub row_head_columns: i32, pub head: Vec<Row>, pub body: Vec<Row> }
pub struct TableFoot { pub attr: Attr, pub rows: Vec<Row> }
pub struct Row { pub attr: Attr, pub cells: Vec<Cell> }
pub struct Cell { pub attr: Attr, pub align: Alignment, pub row_span: i32, pub col_span: i32, pub content: Vec<Block> }
```

Numeric fields use `i32` (mirrors pandoc's `Int` for these bounded quantities); the Value-gate
catches any out-of-range surprise. Derive `Debug, Clone, PartialEq` on every type
(`f64` is `PartialEq`; no `Eq`).

### 3.4 Serde strategy (per type)

- **`Block`, `Inline`, `MetaValue`, and the small enums** (`QuoteType`, `MathType`,
  `ListNumberStyle`, `ListNumberDelim`, `Alignment`, `ColWidth`, `CitationMode`): derive
  `Serialize`/`Deserialize` with `#[serde(tag = "t", content = "c")]`. Verified: nullary variants
  emit `{"t":"X"}` (no `c`), multi-field variants emit `c` as an array. Variant names match Tag
  names exactly (no rename).
- **`Citation`**: derive with `#[serde(rename_all = "camelCase", deny_unknown_fields)]`. Field order
  matches §1.
- **Array-shaped structs** (`Attr`, `Target`, `ListAttributes`, `Caption`, `ColSpec`, `TableHead`,
  `TableBody`, `TableFoot`, `Row`, `Cell`): named fields for readability, but the wire form is a
  fixed-length JSON array, so hand-write `Serialize` (via `serializer.serialize_tuple`) and
  `Deserialize` (via a `Visitor` over `SeqAccess`) in `serde_impls.rs`. `Caption.short` serializes
  `None` as JSON `null`. These manual impls are small and mechanical; keep them together.
- **`Format`**: newtype over `Text`; `#[serde(transparent)]` so it is a bare JSON string.
- **`Document`**: hand-write `Serialize`/`Deserialize`. Serialize a 3-key map in order
  `API_VERSION_KEY`, `"meta"`, `"blocks"` (use `serialize_map` with explicit key order, or a
  `serialize_struct` whose first field name is built from the constant — verify the constant can be
  used; if serde's derive `rename` cannot take a const, the manual impl is required, which is why we
  hand-write it). Deserialize accepts the three keys, errors on a missing api-version, stores the
  version array verbatim.
- **`deny_unknown_fields`** where derive supports it (the structs). For the tagged enums it may not
  compose; do not rely on it there — the §6 Value-equality gate is the universal data-loss guard
  (a dropped field changes the re-serialized `Value`).

### 3.5 Verification while building the AST

After the types compile, before the harness exists, add unit tests in `carta-ast` that assert
exact byte output for a couple of the committed fixtures (§7) — these catch tagging/field-order
mistakes early with readable diffs. (Byte equality is fine *here* because we author these inputs in
canonical form; the corpus gate in §6 stays Value-based.)

## 4. `carta-core` — error type

```rust
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),
}

pub type Result<T> = std::result::Result<T, Error>;
```

Add `thiserror` to workspace deps. Keep this minimal; slice 1 extends it.

## 5. JSON codec + CLI

### 5.1 Codec

The JSON reader/writer are thin and live where slice 1 expects readers/writers, but for slice 0
the simplest home is small functions. Recommended: put `from_json(&[u8]) -> Result<Document>` and
`to_json_writer<W: Write>(&Document, W) -> Result<()>` in `carta-ast` (it already owns serde), and
have the CLI call them. The writer uses `serde_json::to_writer` (compact) then writes a trailing
`\n`. Idiomatic ryu floats — no custom number formatting.

### 5.2 CLI (`carta-cli`)

Add `clap` (derive API). Minimal surface:

```
carta [--from/-f <FORMAT>] [--to/-t <FORMAT>] [-o/--output <FILE>] [INPUT]
```

- `--from`/`--to` default to nothing; slice 0 requires both to be `json`. Any other value →
  `Error::UnsupportedFormat` (the `todo!` for real format dispatch lives behind this, but return an
  error rather than panic on the shipped path).
- `INPUT` absent → read stdin; present → read that file.
- `-o` absent → write stdout; present → write that file.
- Pipeline: read bytes → `from_json` → `to_json_writer`. No filters.
- `main` returns `Result`/exits non-zero on error, printing the error to stderr. No panics.

## 6. Differential round-trip harness (`carta-testkit`)

### 6.1 Minting golden JSON

- Enumerate `*.native` files recursively under `pandoc_tests_dir()` (`.oracle/tests/test`).
- For each, run `<pandoc_bin> -f native -t json <file>`. This is reader-agnostic: the input is
  already an AST, so the round-trip tests our codec, not a reader.
- A `.native` file the binary itself rejects (intentionally malformed corpus entries) is **skipped
  and logged**, not failed.
- **Cache** minted JSON under `.oracle/cache/native-json/<hash>.json`, keyed by
  `sha256(file bytes) + pandoc version`, so reruns don't re-spawn pandoc. Cache dir is gitignored
  (already covered by `/.oracle/`).

### 6.2 The gate

Hard-require the corpus: if `pandoc_tests_dir()` is absent, the round-trip test **fails** with a
message pointing at `tools/install-pandoc.sh` + `tools/fetch-pandoc-tests.sh` (not a silent skip).

For each minted golden JSON `g`:
1. `doc = from_json(g)?` — must succeed (a parse error is a real failure: a tag/shape we don't
   model).
2. `out = to_json(&doc)?`.
3. `assert_eq!(serde_json::from_slice::<Value>(g)?, serde_json::from_slice::<Value>(&out)?)`.

`Value`-equality is float-format-agnostic, key-order-agnostic, and catches dropped fields (present
in `g`'s `Value`, missing in `out`'s). On failure, report the file path and the first differing
JSON pointer to make divergences localizable.

### 6.3 Tag-coverage assertion (§2)

A separate test walks every minted golden `Value`, collecting all `t` strings (recursively, and the
synthetic `MetaMap`/etc. tags inside `meta`). Assert the seen set (union with the committed
fixtures' tags) equals the known set in §2. Fail listing missing tags (add a fixture) **and**
unexpected tags (our model is stale).

### 6.4 Test layout

Round-trip and coverage tests are integration tests in `carta-testkit`
(`crates/carta-testkit/tests/`). They use the existing path helpers (`oracle_dir`, `pandoc_bin`,
`pandoc_tests_dir`). Keep the minting + caching + comparison logic in library functions so tests
stay declarative.

## 7. Committed hand-authored fixtures (offline, day 1)

A handful of `*.json` inputs under `crates/carta-testkit/fixtures/roundtrip/` (committed; these are
authored *inputs*, not pandoc-minted golden output, so they don't violate the "no generated
fixtures committed" rule). Each is canonical pandoc-shaped JSON we write by hand, matching the
shapes in §1. Cover at minimum: an empty doc, a meta block exercising all 6 `MetaValue` variants,
an inline-heavy paragraph, every block kind, a table with `ColWidth` floats, a citation, raw
inline/block.

Offline test (no oracle): for each fixture, `Value(input) == Value(to_json(from_json(input)))`.
This keeps a meaningful round-trip green for agents/CI without the corpus, and gives readable
regression cases. These fixtures also feed the §6.3 coverage union so locally-unreachable tags
(e.g. `MetaString`) are covered deliberately.

## 8. CI

Because the corpus is hard-required, the workspace test run needs `.oracle` provisioned. Update
`.github/workflows/ci.yml`:

- Add a step before tests that runs `tools/install-pandoc.sh` and `tools/fetch-pandoc-tests.sh`,
  wrapped in `actions/cache` keyed on the pinned version (`.oracle/PANDOC_VERSION` +
  `.oracle/TESTS_TAG`) so the pandoc download + corpus clone are cached across runs.
- The offline fixture tests (§7) need no oracle and run regardless.

If provisioning pandoc in CI proves heavy, the fallback is a dedicated `differential` job that does
the provisioning while the main job runs the offline subset — but default to the single provisioned
job unless CI time forces the split.

## 9. Work breakdown (suggested commit sequence, Conventional Commits)

1. `feat(ast): document model types and tag set` — §3.1–3.3 types + derives, no serde yet.
2. `feat(ast): json (de)serialization for the document model` — §3.4 serde (derive + manual array
   impls + Document impl + api-version constant) and §3.5 unit byte-tests.
3. `feat(core): minimal error type` — §4.
4. `feat(cli): json-to-json conversion via clap` — §5.
5. `test(testkit): native-corpus round-trip and tag-coverage harness` — §6 + §7 fixtures.
6. `ci: provision the oracle for differential tests` — §8.

Each commit builds and is clippy/fmt clean. Keep `todo!`s only at the two sanctioned sites (§0.5).

## 10. Risks / watch-items

- **serde adjacent-tagging fidelity.** Verify empirically (round-trip gate) that every variant
  matches the wire shape, especially single-field-that-is-a-tuple cases. Hand-roll the specific
  enum impl if any diverges.
- **`deny_unknown_fields` vs tagged enums** may not compile/compose; the Value-gate is the real
  guard, so don't block on it.
- **Locally-unreachable tags** in the corpus (`MetaString`, `MetaBool`, exotic list styles): covered
  by committed fixtures so the coverage union is complete.
- **api-version key constant vs serde derive rename** — derive `rename` needs a literal; this is the
  reason `Document` gets a manual impl. Don't reintroduce the literal elsewhere.
