# Network Rail SCHEDULE → a Markdown timetable

Turn the Network Rail **"Daily all-TOC snapshot"** (the national rail SCHEDULE
feed) into a readable list of every train you can catch from a station, grouped
by destination. It's an interop showcase that takes the **boundary-vs-engine**
pattern to its limit: even the streaming boundary is Pyfun — there is **no Python
helper module**, just `extern` bindings to `open` / `gzip.open` / `pathlib`.

## Run it

Out of the box it runs against the bundled `sample.ndjson` (a small synthetic
fixture — no data download or account needed):

```sh
cargo run -- run examples/interop/network-rail/chippenham.pyfun
```

(or `cargo run -- compile … -o out.py && python out.py`). It writes
`chippenham-routes.md`, which against the sample looks like:

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
`SCHEDULE` "all-TOC full" snapshot, and point `dataFile` in `chippenham.pyfun` at
it — the **exact** filename, a plain `.json` **or** a `.json.gz` (a `.gz` is
detected by extension and streamed without decompressing). Expect a minute or two
on the full feed: it streams in constant memory, but the ~20-million-line loop
runs through Pyfun's `Seq.fold` rather than a native Python loop.

To list a different station, change the `CHIPNHM` TIPLOC in the `fromChippenham`
guard and the `ingest` prefilter.

## How it works — all in one `.pyfun`

- **The stream** — `open`/`gzip.open` return a file object, which Pyfun consumes
  directly as a lazy `Seq string` of lines (`extern openText : string -> string ->
  Seq string = builtins.open`). `Seq.fold` reduces it in constant memory, so file
  size doesn't matter. A substring prefilter (`String.contains`) skips JSON
  parsing on the lines that can't match.
- **The decode** — the `Decode` module (Elm-style, "parse, don't validate") turns
  each surviving line into typed tuples: no dictionary-poking, and a malformed
  line is a caught `Error`, not a crash.
- **The engine** — public-call detection, downstream slicing, any-calling-point
  destinations (so a Chippenham→Penzance train also counts as a direct service to
  Bath Spa), dedup, title-casing, and Markdown rendering — ordinary Pyfun over
  records, `match`, and `List`/`Map`/`Seq`/`String`/`Option`.

No keyword-argument support is needed at the `extern` boundary: `open`'s mode and
`write_text`'s encoding are ordinary positional arguments (`… "rt"`, `… "utf-8"`).

## Notes

- **"Direct" = one schedule.** Associations (join/split/portion-working) are
  deliberately ignored, which is exactly what makes these no-change journeys.
- **STP caveat.** Counts reflect the day-of-week pattern of permanent + new
  schedules minus full cancellations (`CIF_stp_indicator == "C"`), deduped by
  (departure, destination). A specific calendar date can differ. A full snapshot
  is all `Create` records, so transaction type isn't checked.
- **The bundled fixture is synthetic**, not real feed data — it avoids any
  redistribution question and is built to exercise every branch (a passing-only
  Chippenham, a train terminating there, a mid-route passing point, a deduped
  duplicate, an STP cancellation, and through-services to further termini).
