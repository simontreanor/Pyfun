# Benchmarks

Compute-bound benchmarks for measuring Pyfun's emitted code against hand-written
Python, and for measuring alternative ways of *running* the output (CPython
versions, GraalPy, PyPy via a future `--target 3.11`, mypyc-compiled — see
`ROADMAP.md`, "Performance beyond CPython").

Each benchmark exists twice: `<name>.pyfun` (compiled by the runner to
`out/<name>.py`) and `<name>_baseline.py`, the program a Pythonista would write
by hand for the same job. The baseline is the **ceiling reference**: emitted
code at 1.0x has reached "as fast as the Python you would have written".
Both sides must print byte-identical output — the runner refuses to report
timings for programs that computed different results.

| benchmark | shape | what it stresses |
|---|---|---|
| `expr_eval` | build/simplify/evaluate ADT expression trees | ADT allocation, deep + nested pattern matching |
| `collatz` | total stopping times, recursive | function-call + integer-op throughput (baseline is iterative, as a Pythonista would write it) |
| `map_build` | fold a 500k-insert string-keyed Map, then 500k lookups | the fold-pass dict lowering, `Map.tryFind`/`Option.withDefault`, f-string keys |

Deliberately **not** here: I/O-bound workloads. The network-rail example's
runtime is gzip decompression and long-line scanning, costs every runtime pays
alike; it measures the shape of that job, not the language (see
`local/article-draft-leverage-dont-emulate.md`).

## Running

```bash
python bench/run.py                  # all benchmarks, current CPython
python bench/run.py --bench collatz  # one benchmark
python bench/run.py --python graalpy # time both sides on another interpreter
python bench/run.py --runs 10        # more samples
python bench/run.py --skip-compile   # reuse out/*.py (e.g. hand-edited for a mypyc experiment)
```

The runner uses `target/release/pyfun` or `target/debug/pyfun` if built,
falling back to `cargo run`. Compile time is not measured; only the timed runs
of the resulting programs are.

## Method

Wall-clock, subprocess-level, median of N runs (default 5) after one warmup,
with min..max spread reported so drift is visible. No call-graph profilers:
cProfile charges fixed per-call instrumentation to every function, which makes
trivial functions called millions of times look hot when they aren't
(documented in `ROADMAP.md`, "Further lowering tiers"). Interpreter startup
(~30ms) is included in every measurement on both sides equally; workloads are
sized so it is noise.

When quoting numbers in docs or articles: pin one machine, note the
interpreter line the runner prints, and re-run both sides in the same session.
