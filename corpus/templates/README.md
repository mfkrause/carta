# Template corpus

Self-contained, carta-authored templates that exercise the standalone template language: variables
and nested fields, conditionals, loops and separators, pipes, partials, the whitespace and
indentation rules, and metadata rendered through the target writer (escaping, markup, precedence).

Each `<case>/` directory holds:

- `doc.<ext>`: the entry template. Its extension drives partial resolution, so any partials beside
  it (`<name>.<ext>`) share that extension. The conformance `templates` surface picks up the lone
  `doc.*` file as the entry point.
- `input.md`: the body and any document metadata (a YAML frontmatter block), read on stdin as
  Markdown. Optional; a case with no body may omit it.
- `flags`: optional extra CLI arguments shared by both binaries (`-V`, `-M`, `--metadata-file`).
  The token `@CASE@` is replaced with the absolute case directory, so a metadata file beside the
  template can be referenced as `--metadata-file=@CASE@/extra.yaml`.
- `skip-targets`: optional list (one target per line) of targets this case cannot yet reach
  byte-for-byte; those pairs are skipped and counted rather than silently dropped.

These are authored inputs: one template, body, and flag set drive every supported target, so a
single case exercises the engine, pipes, whitespace rules, and metadata rendering across formats at
once. Templates here deliberately keep control directives flush to the left margin (an indented
control directive opens a layout nesting the language does not otherwise expose) and use scalar
metadata for interpolated fields.
