#!/usr/bin/env python3
"""Wall-clock benchmark runner for Pyfun's compute-bound benchmarks.

Times each compiled Pyfun program against its hand-written Python baseline
("the Python you would have written by hand" -- the ceiling reference).
Wall-clock only, median of N runs after a warmup: call-graph profilers charge
per-call instrumentation to trivial functions and mislead here (see
ROADMAP.md, "Further lowering tiers").

Usage:
  python bench/run.py                       # all benchmarks, current CPython
  python bench/run.py --bench collatz       # one benchmark
  python bench/run.py --python pypy3        # another interpreter (both sides)
  python bench/run.py --runs 10             # more samples
  python bench/run.py --skip-compile        # reuse bench/out/*.py as-is

The runner compiles <name>.pyfun -> bench/out/<name>.py first (cargo run --
compile), verifies both sides print byte-identical output, then reports the
median, spread, and the pyfun/baseline ratio.
"""

import argparse
import platform
import statistics
import subprocess
import sys
import time
from pathlib import Path

BENCH_DIR = Path(__file__).resolve().parent
REPO_ROOT = BENCH_DIR.parent
OUT_DIR = BENCH_DIR / "out"

BENCHES = ["expr_eval", "collatz", "map_build"]


def find_compiler():
    """Prefer an already-built pyfun binary; fall back to cargo run."""
    exe = ".exe" if platform.system() == "Windows" else ""
    for profile in ("release", "debug"):
        candidate = REPO_ROOT / "target" / profile / f"pyfun{exe}"
        if candidate.exists():
            return [str(candidate)]
    return ["cargo", "run", "--quiet", "--"]


def compile_bench(name, compiler):
    src = BENCH_DIR / f"{name}.pyfun"
    dst = OUT_DIR / f"{name}.py"
    OUT_DIR.mkdir(exist_ok=True)
    result = subprocess.run(
        compiler + ["compile", str(src), "-o", str(dst)],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        sys.exit(
            f"compiling {src.name} failed:\n{result.stdout}{result.stderr}\n"
            "(on Windows a stray pyfun.exe held by the LSP can block cargo: "
            "Get-Process pyfun | Stop-Process -Force)"
        )
    return dst


def run_once(python, script):
    t0 = time.perf_counter()
    result = subprocess.run(
        [python, str(script)], capture_output=True, text=True, cwd=REPO_ROOT
    )
    elapsed = time.perf_counter() - t0
    if result.returncode != 0:
        sys.exit(f"{script.name} failed under {python}:\n{result.stderr}")
    return elapsed, result.stdout


def time_script(python, script, runs, warmup=1):
    times = []
    reference = None
    for i in range(warmup + runs):
        elapsed, out = run_once(python, script)
        if reference is None:
            reference = out
        elif out != reference:
            sys.exit(f"{script.name}: output changed between runs (nondeterministic?)")
        if i >= warmup:
            times.append(elapsed)
    return times, reference


def fmt(seconds):
    return f"{seconds:8.3f}s"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bench", action="append", choices=BENCHES,
                        help="run only this benchmark (repeatable)")
    parser.add_argument("--python", default=sys.executable,
                        help="interpreter to time both sides with")
    parser.add_argument("--runs", type=int, default=5, help="timed runs (default 5)")
    parser.add_argument("--skip-compile", action="store_true",
                        help="reuse existing bench/out/*.py")
    args = parser.parse_args()

    selected = args.bench or BENCHES
    compiler = None if args.skip_compile else find_compiler()

    version = subprocess.run([args.python, "-VV"], capture_output=True, text=True)
    print(f"interpreter: {version.stdout.strip() or args.python}")
    print(f"runs: {args.runs} (median reported), warmup: 1\n")

    header = f"{'benchmark':<12} {'pyfun':>9} {'baseline':>9} {'ratio':>7}   spread (pyfun min..max)"
    print(header)
    print("-" * len(header))

    for name in selected:
        emitted = OUT_DIR / f"{name}.py" if args.skip_compile else compile_bench(name, compiler)
        if not emitted.exists():
            sys.exit(f"{emitted} missing -- run once without --skip-compile")
        baseline = BENCH_DIR / f"{name}_baseline.py"

        py_times, py_out = time_script(args.python, emitted, args.runs)
        base_times, base_out = time_script(args.python, baseline, args.runs)

        if py_out != base_out:
            sys.exit(
                f"{name}: OUTPUT MISMATCH -- the two versions computed different results\n"
                f"  pyfun:    {py_out!r}\n  baseline: {base_out!r}"
            )

        py_med = statistics.median(py_times)
        base_med = statistics.median(base_times)
        ratio = py_med / base_med if base_med > 0 else float("inf")
        print(
            f"{name:<12} {fmt(py_med)} {fmt(base_med)} {ratio:6.2f}x"
            f"   {min(py_times):.3f}..{max(py_times):.3f}"
        )

    print("\noutputs verified identical between pyfun and baseline for each benchmark")


if __name__ == "__main__":
    main()
