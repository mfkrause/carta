# Surveying the river delta

The estuary widens here, where the slow water folds back on itself and the
reeds stand in shallow ranks. A surveyor walking the bank counts the channels
by eye, then again with instruments, and the two tallies rarely agree on the
first pass. *Patience* is the only reliable tool, and **a steady hand** records
what patience finds. The work is unglamorous: stakes, string, a notebook gone
soft at the corners, and the long arithmetic of `mean ± deviation` done twice.

## Methods in the field

Each morning begins with a calibration. We level the instrument against a fixed
mark, note the temperature, and only then begin to read. The procedure matters
less for any single measurement than for the *consistency* it imposes across a
season of them — consistency is what turns a heap of numbers into a record.

A short walk upstream reaches the gauging station, an unremarkable hut with a
remarkable view. Inside, the logbook runs back decades; outside, the [river
gauge](https://example.org/gauge) ticks over in centimetres. Compare the two and
you learn how much a place can change while seeming to stay still.

### What the notebook holds

- Stage height, read to the millimetre and rounded honestly.
- Water temperature, because density is not a constant.
- Weather, in plain words: *overcast*, *gusting*, *still*.
- Anything unusual, however small — a smell, a colour, a drifting log.

The list is short on purpose. A field notebook that asks for everything gets
nothing; one that asks for little gets filled. The discipline is in the asking.

When the season turns, the entries grow denser:

1. First the routine readings, taken at the same hour each day.
2. Then the supplementary set, taken whenever the stage moves sharply.
3. Finally the reconciliation, where yesterday's figures are checked against
   today's and the discrepancies are chased down one by one.

> The river does not keep our hours, and it does not round to the millimetre.
> What we record is a negotiation between the water's indifference and our need
> to write something down. The honest surveyor admits as much in the margin.

## A note on instruments

Two instruments, nominally identical, will disagree. The disagreement is not a
defect but a measurement in its own right — of drift, of wear, of the small
violences of transport. We keep both, and we keep their quarrel in the record:

```rust
fn reconcile(a: f64, b: f64) -> Reading {
    let mean = (a + b) / 2.0;
    let spread = (a - b).abs();
    Reading { mean, spread, trustworthy: spread < TOLERANCE }
}
```

The indented form, older and stubborn, still appears in the archived sheets:

    stage = datum + offset
    flow  = stage * width * velocity

Read an autolink as what it is: <https://example.org/archive>. Read an entity
as the character it names — a fraction &frac12;, a dash &mdash; set off mid
sentence, an ampersand &amp; standing on its own. The text means what it says.

A photograph helps where words fail: ![weir at low water](weir.jpg). It is not
evidence, exactly, but it is a witness, and witnesses are worth keeping.

---

Close the notebook gently. The corner is soft, the string is wound, the stakes
are pulled and stacked. Tomorrow the river will be a little different and the
arithmetic will start again, twice, until the two tallies agree.
