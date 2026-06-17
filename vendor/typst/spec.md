# Typst Syntax Reference

This document captures the public Typst markup syntax reference, the format specification that
drives this direction's writer. It is condensed from the official reference documentation at
<https://typst.app/docs/reference/syntax/> and the symbols reference at
<https://typst.app/docs/reference/symbols/>, pinned to the version recorded in `VERSION`. See
`ATTRIBUTION.md` for provenance and licensing.

## Modes

Typst source is parsed in one of three modes, which nest into one another:

- **Markup mode** is the default at the top of a document. Text is literal; a small set of
  special characters introduce structure. A markup block can be opened anywhere with `[..]`.
- **Code mode** is entered with a leading `#` (e.g. `#(1 + 2)`, `#rect(width: 1cm)`) and is used
  for expressions, bindings, control flow, and styling rules.
- **Math mode** is entered by surrounding content with `$..$`. Block math pads the dollars with
  spaces (`$ x^2 $`); inline math omits them (`$x^2$`).

## Markup

| Element          | Syntax                  | Element function |
| ---------------- | ----------------------- | ---------------- |
| Paragraph break  | Blank line              | `parbreak`       |
| Strong emphasis  | `*strong*`              | `strong`         |
| Emphasis         | `_emphasis_`            | `emph`           |
| Raw text         | `` `code` ``            | `raw`            |
| Link             | `https://typst.app/`    | `link`           |
| Label            | `<intro>`               | `label`          |
| Reference        | `@intro`                | `ref`            |
| Heading          | `= Heading`             | `heading`        |
| Bullet list      | `- item`                | `list`           |
| Numbered list    | `+ item`                | `enum`           |
| Term list        | `/ Term: description`   | `terms`          |
| Math             | `$x^2$`                 | (math mode)      |
| Line break       | `\`                     | `linebreak`      |
| Smart quote      | `'single'` or `"double"`| `smartquote`     |
| Symbol shorthand | `~`, `---`              | (symbols)        |
| Code expression  | `#rect(width: 1cm)`     | (code mode)      |
| Character escape  | `\#`, `\*`             | (escapes)        |
| Comment          | `// line`, `/* block */`| (comments)       |

Notes on the markup elements:

- **Headings.** The number of leading `=` signs sets the level: `=` is level 1, `==` is level 2,
  and so on. The marker must be followed by a space.
- **Lists.** A `-` opens a bullet item, a `+` opens a numbered item, and `/ Term: description`
  opens a term-list item. Nesting is by indentation. An explicit number can be given as
  `2. item` to start an enumeration at a particular value.
- **Strong and emphasis** nest and can span multiple words: `*This is _all_ strong.*`
- **Raw.** A single backtick pair is inline raw text. A triple-backtick fence is a raw block and
  may carry a language tag, e.g. ```` ```rust ````.
- **Links.** A bare URL in markup becomes a link automatically.
- **Labels and references.** `<name>` attaches a label to the preceding element; `@name`
  references it.

## Shorthands

In markup mode the following ASCII sequences expand to the listed characters:

| Shorthand | Produces                  |
| --------- | ------------------------- |
| `~`       | non-breaking space        |
| `-?`      | soft hyphen               |
| `--`      | en dash (–)               |
| `---`     | em dash (—)               |
| `...`     | ellipsis (…)              |

Math mode adds shorthands for arrows and operators, including `->`, `<-`, `=>`, `<=>`, `-->`,
`<--`, `|->`, `!=`, `<=`, `>=`, `<<`, `>>`, `:=`, and `...`. Any shorthand can be deactivated by
escaping its first character.

## Escapes

A backslash escapes the following character so it is treated literally rather than as markup, e.g.
`\#`, `\*`, `\_`, `\` followed by a backtick, `\<`, `\@`. A Unicode codepoint can be inserted with
`\u{..}` giving the codepoint in hexadecimal, e.g. `\u{1f600}`.

## Comments

- Line comment: `// to end of line`
- Block comment: `/* ... */` (may span multiple lines and nests)

## Identifiers and paths

- Identifiers start with a letter or underscore and may contain letters, digits, hyphens, and
  underscores. Kebab-case is the conventional style (`top-edge`).
- Resource paths are relative to the current file (`image("images/logo.png")`) or absolute from
  the project root (`image("/assets/logo.png")`).

## Code mode (overview)

Code mode expressions cover literals (`none`, `auto`, booleans, integers, floats, strings, and
typed quantities such as lengths `1cm`, angles `90deg`, ratios `50%`, and fractions `1fr`),
operators, field access, method and function calls, control flow (`if`/`else`, `for`, `while`,
`break`, `continue`), bindings (`let`), function definitions, set and show rules for styling, and
module `import`/`include`. A function call in markup is written `#name(args)`; content arguments may
be passed as a trailing markup block, e.g. `#figure[..]`.
