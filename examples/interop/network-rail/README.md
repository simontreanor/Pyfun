# Network Rail SCHEDULE → a Markdown timetable

Turn the Network Rail **"Daily all-TOC snapshot"** (the national rail SCHEDULE
feed) into a readable list of every train you can catch from a station, grouped
by destination. It ships as **two versions of the same program** — identical
output, different streaming boundary — and the contrast between them is the
whole point (see [Two versions, one lesson](#two-versions-one-lesson)).

| File | Streaming boundary | Real-feed speed | Run with |
|------|--------------------|-----------------|----------|
| `chippenham.pyfun` | pure Pyfun (`Seq.fold` over the file) | ~75 s | `pyfun run` (no `PYTHONPATH`) |
| `chippenham_fast.pyfun` + `nr_stream.py` | native Python loop | ~5 s | `compile` + `PYTHONPATH` |

The domain engine below the boundary is byte-for-byte identical in both.

## Run it

Both run out of the box against the bundled `sample.ndjson` (a small synthetic
fixture — no data download or account needed) and write `chippenham-routes.md`:

```sh
# pure Pyfun — even the stream is Pyfun
cargo run -- run examples/interop/network-rail/chippenham.pyfun

# Python-helper variant — needs the module on PYTHONPATH
cargo run -- compile examples/interop/network-rail/chippenham_fast.pyfun \
    -o examples/interop/network-rail/chippenham_fast.py
PYTHONPATH=examples/interop/network-rail \
    python examples/interop/network-rail/chippenham_fast.py
```

Against the sample the report looks like:

```markdown
## Monday–Friday (representative day: Tuesday)

### Bath Spa — 1 train, 11 min

- **10:02 → 10:13** · 11 min · direct _(to Bristol Temple Meads)_

### Westbury — 1 train, 28 min

- **11:17 → 11:45** · 28 min · via Melksham, Trowbridge
```

A destination header shows the train count and the fastest–slowest journey
time; each service line is `departure → arrival · journey · calling points`,
with `_(to …)_` when the train runs *through* this station to a further
terminus. Output is split into weekday / Saturday / Sunday sections.

## Run it against the real feed

The real snapshot is newline-delimited JSON, ~3 GB uncompressed (≈40 MB gzip),
one schedule object per line. Register for the data feeds
(<https://opendata.nationalrail.co.uk> / Network Rail data feeds), download a
`SCHEDULE` "all-TOC full" snapshot, and point `dataFile` at it — a plain `.json`
**or** a `.json.gz` (streamed without decompressing). To list a different
station, change the `CHIPNHM` TIPLOC.

## Two versions, one lesson

Both versions stream the file in constant memory and produce identical output.
The only difference is *where the ~20-million-line hot loop runs*:

- **`chippenham_fast.pyfun`** hands the loop to `nr_stream.py`, a tight, C-backed
  Python `for` loop that substring-filters each line before parsing. ~5 s.
- **`chippenham.pyfun`** does the loop in Pyfun: the file object is consumed
  directly as a lazy `Seq string` and reduced with `Seq.fold`, paying one Pyfun
  function call per line. ~75 s — about 15× slower.

That gap **is the argument for Pyfun's design**. You *can* do the whole thing in
Pyfun, with no Python at all — and it's a lovely demonstration that you can (lazy
`Seq` over an `extern` iterator, `Decode` instead of dict-poking, positional
`extern` args for `open`/`write_text`). But you *shouldn't*: a hot per-byte loop
is exactly what decades of optimised CPython are for. Pyfun isn't trying to
re-implement Python's runtime — it's a **typed engine that sits on top of it**,
and `extern` is the seam that lets the typed part stay small and safe while the
grinding is delegated to the fast thing that already exists. Leverage, don't
emulate. The right architecture is usually the hybrid: Python at the boundary,
Pyfun for the logic you actually want to get right.

## How the pieces fit

- **The boundary** — `open`/`gzip.open` return a file object. The fast version
  iterates it in Python; the pure version types it as `Seq string` and folds it.
  Either way memory stays flat regardless of file size.
- **The decode** — the pure version uses the `Decode` module (Elm-style, "parse,
  don't validate") to turn each surviving line into typed tuples; the fast
  version does the dict navigation in Python and hands typed tuples across.
- **The engine** — public-call detection, downstream slicing, any-calling-point
  destinations (so a Chippenham→Penzance train also counts as a direct service to
  Bath Spa), dedup, title-casing, and Markdown rendering — ordinary Pyfun over
  records, `match`, and `List`/`Map`/`Seq`/`String`/`Option`.

## Notes

- **"Direct" = one schedule.** Associations (join/split/portion-working) are
  deliberately ignored, which is what makes these no-change journeys.
- **STP caveat.** Counts reflect the day-of-week pattern of permanent + new
  schedules minus full cancellations (`CIF_stp_indicator == "C"`), deduped by
  (departure, destination). A specific calendar date can differ.
- **The bundled fixture is synthetic**, not real feed data — it avoids any
  redistribution question and is built to exercise every branch (a passing-only
  Chippenham, a train terminating there, a mid-route passing point, a deduped
  duplicate, an STP cancellation, and through-services to further termini).
