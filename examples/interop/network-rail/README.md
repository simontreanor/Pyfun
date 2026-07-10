# Network Rail SCHEDULE → a Markdown timetable

Turn the Network Rail **"Daily all-TOC snapshot"** (the national rail SCHEDULE
feed) into a readable list of every train you can catch from a station, grouped
by destination. It ships as **two versions of the same program** with a
byte-identical domain engine and identical output — they differ only in *where
the streaming boundary lives*, and the story of how their performance converged
is the interesting part (see [Two versions, one lesson](#two-versions-one-lesson)).

| File | Streaming boundary | Real-feed speed | Run with |
|------|--------------------|-----------------|----------|
| **`chippenham.pyfun`** (recommended) | pure Pyfun (`Seq.fold` over the file) | ~7 s | `pyfun run` (no `PYTHONPATH`) |
| `chippenham_fast.pyfun` + `nr_stream.py` | native Python loop | ~5 s | `compile` + `PYTHONPATH` |

**Default to the pure version.** It has no Python helper at all — `pyfun run`
just works — is typed end-to-end, and turns a malformed line into a caught
`Error` rather than a crash. On the full feed it's only ~1.3× slower than the
hand-written Python loop (≈2 s on a once-a-day report — noise). Reach for the
`_fast` variant only when raw throughput genuinely matters.

## Run it

Both run out of the box against the bundled `sample.ndjson` (a small synthetic
fixture — no data download or account needed) and write `chippenham-routes.md`:

```sh
# pure Pyfun — even the stream is Pyfun; no helper, no PYTHONPATH
cargo run -- run examples/interop/network-rail/chippenham.pyfun

# throughput variant — the streaming loop is native Python (needs the module on PYTHONPATH)
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
~660k lines, one schedule object per line. Register for the data feeds
(<https://opendata.nationalrail.co.uk> / Network Rail data feeds), download a
`SCHEDULE` "all-TOC full" snapshot, and point `dataFile` at it — a plain `.json`
**or** a `.json.gz` (streamed without decompressing). To list a different
station, change the `CHIPNHM` TIPLOC.

## Two versions, one lesson

Both stream the file in constant memory and produce **byte-identical output**.
The only difference is *where the ~660k-line hot loop runs*:

- **`chippenham_fast.pyfun`** hands the loop to `nr_stream.py`: a tight, C-backed
  Python `for` loop that substring-filters each line before parsing (~5 s).
- **`chippenham.pyfun`** does the loop in Pyfun — the file object is consumed
  directly as a lazy `Seq string` and reduced with `Seq.fold` (~7 s).

The interesting part is that these numbers used to be **~15× apart**, and closing
that gap is the real lesson — a more useful one than "don't do it in your own
language":

1. **Profile before you conclude.** The naive pure version was ~15× slower, which
   looks like a damning native-vs-Pyfun gap. It wasn't: cProfile showed ~87% of
   the time in one line — `Map.add` lowering to a full `dict` *copy per insert*,
   so building the ~12k-entry TIPLOC table in the fold was accidentally O(n²).
   Plus the file was being read as the platform locale (cp1252), a slow decode.
2. **When the gap is a compiler wart, fix the compiler.** An in-place
   fold-lowering pass (`DESIGN.md` §5.1) rewrites a linear-accumulator `Seq.fold`
   into a `for`-loop with a mutable accumulator (`Map.add`→`m[k]=v`), collapsing
   the O(n²) to linear — and it raises the floor for *every* Pyfun program, not
   just this one. Reading UTF-8 (via pinned `extern` keyword args) fixed the rest.
   Result: ~15× → ~1.3×.
3. **Reserve the boundary for the genuinely-hot residual.** That last ~1.3× is
   real: a native `for` loop with an inline `in` still beats Pyfun's per-line
   `String.contains` calls and `Decode`. So `extern`-to-native is the right move
   when a loop is *actually* hot — not a reflex, and not something to reach for
   before you've measured. (Even this residual isn't a floor: inlining
   `String.contains` and compiling the decoders would shrink it further.)

So Pyfun's pitch isn't "reimplement Python's runtime" — it's a **typed engine on
top of it**, with `extern` as the seam to the fast thing that already exists.
Leverage, don't emulate — but first check whether the gap is a wart you can file
away, and let the measurement, not a slogan, decide where the boundary goes.

## How the pieces fit

- **The boundary** — `open`/`gzip.open` return a file object. The fast version
  iterates it in Python; the pure version types it as `Seq string` and folds it.
  Either way memory stays flat regardless of file size. The pure version pins
  `mode`/`encoding`/`newline` as `extern` keyword arguments, so the emitted Python
  reads and writes UTF-8 with no positional filler.
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
