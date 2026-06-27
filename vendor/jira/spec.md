# Jira Wiki Markup Reference

This document captures the public Jira wiki markup syntax — the lightweight "text formatting
notation" used in Jira issue fields and comments — as the format specification that drives this
direction's reader. It is condensed from the public reference page at
<https://jira.atlassian.com/secure/WikiRendererHelpAction.jspa?section=all>, pinned to the
reference capture recorded in `VERSION`. See `ATTRIBUTION.md` for provenance and licensing.

The text is an original, reformatted summary authored for this repository. It records the markup
tokens (which are functional notation, not creative prose) and the structure they produce; it is
not a verbatim copy of the upstream documentation.

## Document model

Wiki markup is line-oriented. Most block constructs are recognised by a prefix at the start of a
line (`h1.`, `bq.`, `*`, `#`, `||`) or by a paired brace macro (`{code}…{code}`). Blocks are
separated by one or more blank lines. Within a block, inline markup (emphasis, links, images,
monospaced spans, …) is applied to the run of text.

## Headings

A line beginning with `hN.` followed by a space starts a heading of level `N`, where `N` is `1`
through `6`. The heading text runs to the end of the line.

| Notation | Result |
| -------- | ------ |
| `h1. Title` | level-1 heading |
| `h2. Title` | level-2 heading |
| `h3. Title` | level-3 heading |
| `h4. Title` | level-4 heading |
| `h5. Title` | level-5 heading |
| `h6. Title` | level-6 heading |

## Text effects

Inline effects wrap a span of text in matched delimiters.

| Notation | Effect |
| -------- | ------ |
| `*strong*` | strong / bold |
| `_emphasis_` | emphasis / italic |
| `??citation??` | citation |
| `-deleted-` | struck-through (deleted) |
| `+inserted+` | underlined (inserted) |
| `^superscript^` | superscript |
| `~subscript~` | subscript |
| `{{monospaced}}` | monospaced (fixed-width) span |
| `{color:red}text{color}` | coloured text; the argument is a colour name or `#rrggbb` hex value |

## Text breaks

| Notation | Effect |
| -------- | ------ |
| blank line | paragraph break |
| `\\` | forced line break within a paragraph |
| `----` | horizontal rule (four hyphens alone on a line) |
| `---` | em dash (—) |
| `--` | en dash (–) |

## Block quotes

| Notation | Effect |
| -------- | ------ |
| `bq. quoted line` | quotes a single line |
| `{quote}…{quote}` | quotes the enclosed block(s), spanning multiple paragraphs |

## Lists

A list item is introduced by one or more marker characters at the start of a line, followed by a
space. The marker characters are:

- `*` — bulleted item
- `-` — bulleted item (alternate marker)
- `#` — numbered item

Nesting depth is the number of leading marker characters; the marker at each position selects that
level's list type, so markers may be mixed to interleave bulleted and numbered levels.

| Notation | Effect |
| -------- | ------ |
| `* item` | first-level bullet |
| `** item` | second-level bullet |
| `# item` | first-level numbered item |
| `## item` | second-level numbered item |
| `*# item` | numbered item nested under a bullet |
| `#* item` | bullet nested under a numbered item |

## Links

A link is written inside square brackets. An optional label precedes the target, separated by a
pipe; an optional tooltip may follow the target after a second pipe.

| Notation | Target |
| -------- | ------ |
| `[http://example.com]` | external URL (the URL is shown as the label) |
| `[label\|http://example.com]` | external URL with a label |
| `[label\|http://example.com\|tooltip]` | external URL with a label and hover tooltip |
| `[mailto:user@example.com]` | email link |
| `[file:///c:/path/file.txt]` | local file link |
| `[#anchor]` | link to a named anchor on the same page |
| `[label\|#anchor]` | labelled link to an anchor |
| `[^attachment.ext]` | link to a file attached to the issue |
| `[~username]` | link to a user's profile |

## Anchors

| Notation | Effect |
| -------- | ------ |
| `{anchor:name}` | defines a named anchor that `[#name]` links to |

## Images and attachments

An image is written between exclamation marks. The source may be an attachment file name or an
absolute URL. Comma-separated properties may follow a pipe.

| Notation | Effect |
| -------- | ------ |
| `!image.gif!` | embeds an attached image |
| `!http://host/image.gif!` | embeds a remote image |
| `!image.jpg\|thumbnail!` | embeds a thumbnail that links to the full image |
| `!image.gif\|align=right, vspace=4!` | embeds with HTML-style properties |

Recognised image properties include `thumbnail`, `align`, `border`, `bordercolor`, `hspace`,
`vspace`, `width`, `height`, `title`, and `alt`.

## Tables

A table is a run of consecutive lines, each a row. Header cells are delimited by `||`; body cells
by `|`. A row may mix header and body cells.

| Notation | Effect |
| -------- | ------ |
| `\|\|heading 1\|\|heading 2\|\|` | a header row |
| `\|cell 1\|cell 2\|` | a body row |

## Preformatted and code blocks

These paired brace macros enclose a block of literal text.

| Notation | Effect |
| -------- | ------ |
| `{noformat}…{noformat}` | preformatted block; inline markup is not interpreted |
| `{code}…{code}` | syntax-highlighted code block |
| `{code:java}…{code}` | code block highlighted for the named language |
| `{code:title=Sample.java\|borderStyle=solid}…{code}` | code block with parameters |

`{code}` accepts a leading language token and/or pipe-separated parameters (`title`, `borderStyle`,
`borderColor`, `linenumbers`, `collapse`, …). Recognised language tokens include: `actionscript`,
`ada`, `applescript`, `bash`, `c`, `c#`, `c++`, `css`, `erlang`, `go`, `groovy`, `haskell`, `html`,
`java`, `javascript`, `json`, `lua`, `none`, `nyan`, `objc`, `perl`, `php`, `python`, `r`, `ruby`,
`scala`, `sql`, `swift`, `visualbasic`, `xml`, and `yaml`.

## Panels

| Notation | Effect |
| -------- | ------ |
| `{panel}…{panel}` | bordered panel |
| `{panel:title=My panel}…{panel}` | panel with a title bar |

`{panel}` accepts pipe-separated parameters: `title`, `borderStyle`, `borderColor`, `borderWidth`,
`bgColor`, and `titleBGColor`. The same parameters apply to `{noformat}`.

## Escaping

A backslash before a character emits that character literally, suppressing any markup meaning it
would otherwise carry: `\*` produces a literal asterisk.

## Symbols and emoticons

These character sequences are replaced by a symbol or icon.

| Notation | Symbol |
| -------- | ------ |
| `(!)` | warning |
| `(x)` | error / cross |
| `(/)` | check mark |
| `(i)` | information |
| `(?)` | help / question |
| `(y)` | thumbs up |
| `(n)` | thumbs down |
| `(+)` | plus / add |
| `(-)` | minus |
| `(on)` | light bulb on |
| `(off)` | light bulb off |
| `(*)` | star (yellow) |
| `(*r)` | star (red) |
| `(*g)` | star (green) |
| `(*b)` | star (blue) |
| `(*y)` | star (yellow) |
| `(flag)` | flag |
| `(flagoff)` | flag (grey) |
| `:)` `:(` `:P` `:D` `;)` | smiley faces |
