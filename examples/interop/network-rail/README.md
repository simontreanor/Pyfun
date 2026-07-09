# Network Rail SCHEDULE → a Markdown timetable

Turn the Network Rail **"Daily all-TOC snapshot"** (the national rail SCHEDULE
feed) into a readable list of every train you can catch from a station, grouped
by destination. This is Pyfun's **boundary-vs-engine** interop pattern at scale:
a big, messy, untyped data source is streamed and shape-extracted on the Python
side, and the interesting domain logic is a typed, total Pyfun program.

## Run it

Out of the box it runs against the bundled `sample.ndjson` (a small synthetic
fixture — no data download or account needed):

```sh
cargo run -- compile examples/interop/network-rail/chippenham.pyfun \
    -o examples/interop/network-rail/chippenham.py
PYTHONPATH=examples/interop/network-rail \
    python examples/interop/network-rail/chippenham.py
```

It writes `chippenham-routes.md`. Against the sample that looks like:

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
`SCHEDULE` "all-TOC full" snapshot, and point `dataFile` in `chippenham.pyfun`
at it — a plain `.json` **or** a `.json.gz` (the boundary streams the gzip
directly, so you needn't decompress it). Recompile and run; it finishes in a few
seconds because the Python side substring-prefilters before it ever calls
`json.loads`.

To list a different station, change the `CHIPNHM` TIPLOC in both
`chippenham.pyfun` (the `fromChippenham` guard) and `nr_stream.py` (the `CHIPP`
prefilter constant).

## How it works

- **`nr_stream.py`** (the boundary) — streams the file one line at a time,
  gzip-aware, skips lines that don't mention the station with a cheap substring
  test *before* paying for `json.loads`, and returns only `(tiploc_name_map,
  matching_runs)`. Memory stays flat regardless of file size; only a few hundred
  runs cross the `extern` boundary.
- **`chippenham.pyfun`** (the engine) — decides what counts as a *public* call
  (passing points carry no public time), slices each schedule downstream of the
  station, treats **every reachable calling point** as a destination (so a
  Chippenham→Penzance train also counts as a direct service to Bath Spa),
  dedups identical patterns, title-cases the UPPERCASE feed names, and renders
  Markdown. It's ordinary Pyfun — records, `match`, `List`/`Map`/`String`/
  `Option`, and inferred effects — no per-feature compiler support.

## Notes

- **"Direct" = one schedule.** Associations (join/split/portion-working) are
  deliberately ignored, which is exactly what makes these no-change journeys.
- **STP caveat.** Counts reflect the day-of-week pattern of permanent + new
  schedules minus full cancellations (`CIF_stp_indicator == "C"`), deduped by
  (departure, destination) to avoid short-term-plan overlays double-counting. A
  specific calendar date can differ (an overlay or cancellation applies only on
  certain dates); resolving against a real date is left as an exercise.
- **The bundled fixture is synthetic**, not real feed data — it avoids any
  redistribution question and is built to exercise every branch (a passing-only
  Chippenham, a train terminating there, a mid-route passing point, a deduped
  duplicate, an STP cancellation, and through-services to further termini).
