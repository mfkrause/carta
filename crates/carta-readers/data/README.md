# Vendored data

- `entities.json`: the named character reference table from the WHATWG HTML
  standard (<https://html.spec.whatwg.org/entities.json>), the authoritative
  source CommonMark cites for valid entity references. Consumed by `build.rs`,
  which generates a sorted lookup table compiled into the crate.
