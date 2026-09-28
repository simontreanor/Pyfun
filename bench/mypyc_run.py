"""Measure `--native` output compiled with mypyc (ROADMAP: Typed-emit + mypyc AOT).

For each benchmark, time three versions of the same program against the
hand-written baseline, checking all of them print the same output:

  native      `pyfun compile --native`, run by CPython as plain Python
  mypyc       `pyfun build --native`: the same program as a mypyc C extension
  baseline    the hand-written Python in bench/<name>_baseline.py

The emitted program runs its work at import time, so each version is timed as
`python -c "import <module>"` from a scratch directory, which is also the only
way to run a mypyc-compiled module. Needs `mypy` (which ships mypyc) and a C
compiler; on Windows that means the MSVC Build Tools.

Usage:
  PYFUN_BIN=target/release/pyfun python bench/mypyc_run.py [--runs 5]
"""

import argparse
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

BENCH = Path(__file__).resolve().parent
REPO = BENCH.parent
BENCHES = ["expr_eval", "collatz", "map_build"]


def pyfun_bin():
    if os.environ.get("PYFUN_BIN"):
        return os.environ["PYFUN_BIN"]
    exe = ".exe" if os.name == "nt" else ""
    for profile in ("release", "debug"):
        p = REPO / "target" / profile / f"pyfun{exe}"
        if p.exists():
            return str(p)
    sys.exit("build pyfun first (cargo build --release) or set PYFUN_BIN")


def time_import(workdir, module, runs):
    cmd = [sys.executable, "-c", f"import {module}"]
    first = subprocess.run(cmd, cwd=workdir, capture_output=True, text=True)
    if first.returncode != 0:
        raise RuntimeError(f"importing {module} failed:\n{first.stderr[-3000:]}")
    out = first.stdout
    samples = []
    for _ in range(runs):
        start = time.perf_counter()
        subprocess.run(cmd, cwd=workdir, capture_output=True, check=True)
        samples.append(time.perf_counter() - start)
    return statistics.median(samples), out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", type=int, default=5)
    args = ap.parse_args()
    compiler = pyfun_bin()
    print(f"python: {sys.version.split()[0]}   runs: {args.runs} (median)\n")
    print(f"{'benchmark':<11} {'native':>8} {'mypyc':>8} {'baseline':>9}   mypyc vs native   mypyc vs baseline")
    for name in BENCHES:
        work = Path(tempfile.mkdtemp(prefix=f"pyfun_mypyc_{name}_"))
        try:
            native = work / f"{name}_native.py"
            subprocess.run(
                [compiler, "compile", "--native", str(BENCH / f"{name}.pyfun"), "-o", str(native)],
                check=True,
            )
            # The mypyc build is `pyfun build --native`, the command users run.
            built = work / "built"
            build = subprocess.run(
                [compiler, "build", "--native", str(BENCH / f"{name}.pyfun"), "-o", str(built)],
                capture_output=True,
                text=True,
            )
            if build.returncode != 0:
                print(f"{name:<11} pyfun build failed:\n{build.stdout[-2000:]}{build.stderr[-2000:]}")
                continue
            shutil.copy(BENCH / f"{name}_baseline.py", work / f"{name}_base.py")
            try:
                t_native, o_native = time_import(work, f"{name}_native", args.runs)
                t_mypyc, o_mypyc = time_import(built, name, args.runs)
                t_base, o_base = time_import(work, f"{name}_base", args.runs)
            except RuntimeError as err:
                print(f"{name:<11} {err}")
                continue
            if not (o_native == o_mypyc == o_base):
                print(f"{name:<11} OUTPUT MISMATCH")
                continue
            print(
                f"{name:<11} {t_native:>7.3f}s {t_mypyc:>7.3f}s {t_base:>8.3f}s"
                f"   {t_native / t_mypyc:>6.2f}x faster   {t_mypyc / t_base:>6.2f}x of baseline"
            )
        finally:
            shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
