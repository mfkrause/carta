# Vendored data

- `entities.json`: the named character reference table from the WHATWG HTML
  standard (<https://html.spec.whatwg.org/entities.json>). The writers consult
  the semicolon-terminated names to decide when a literal `&` in running text
  must be escaped so it is not re-read as a character reference.

This crate keeps its own copy so it builds in isolation. A test asserts it stays
byte-identical to the readers crate's copy; update both together.
