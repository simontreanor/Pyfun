"""Check that a native build prints what the plain build prints.

Every runnable program in the lessons (docs/src/learn), plus the examples that
need no network or local files, is run with `pyfun run` and then built with
`pyfun build --native` (mypyc) and run again. The two outputs must match, and a
program that runs plainly must also build natively.

Usage: python tests/native_parity.py [-j N]
Needs `pip install mypy setuptools` and a C compiler. The compiler it runs is
`target/debug/pyfun` (override with PYFUN_BIN).
"""

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
LEARN = REPO / "docs" / "src" / "learn"
EXAMPLES = [
    "examples/hello.pyfun",
    "examples/modules/main.pyfun",
    "examples/interop/structured_concurrency.pyfun",
]
# Programs known to fail natively, each with its reason (ROADMAP: Typed-emit +
# mypyc AOT). One that starts passing fails the run until it leaves this list.
KNOWN = {
    "examples/interop/structured_concurrency.pyfun": "a coroutine rebinding a `nonlocal` "
    "crashes when mypyc builds it under --allow-redefinition-new",
}
PYFUN = Path(
    os.environ.get("PYFUN_BIN")
    or next(
        (p for p in (REPO / "target/debug/pyfun.exe", REPO / "target/debug/pyfun") if p.exists()),
        REPO / "target/debug/pyfun",
    )
)
BLOCK_RE = re.compile(r"```pyfun\n(.*?)```", re.DOTALL)


def run(args, cwd=None):
    return subprocess.run(
        args,
        capture_output=True,
        text=True,
        encoding="utf-8",
        stdin=subprocess.DEVNULL,
        cwd=cwd,
        timeout=600,
    )


def corpus(work: Path):
    """(name, path) for every program to compare; lesson blocks are written out."""
    for md in sorted(LEARN.glob("[0-9][0-9]-*.md")):
        for i, block in enumerate(BLOCK_RE.findall(md.read_text(encoding="utf-8"))):
            if re.search(r"^import [A-Z]", block, re.MULTILINE):
                continue  # a file-based module example needs its sibling files
            path = work / f"{md.stem.replace('-', '_')}_{i}.pyfun"
            path.write_text(block, encoding="utf-8", newline="\n")
            yield f"{md.stem} block {i}", path
    for example in EXAMPLES:
        yield example, REPO / example


def compare(item, work: Path):
    name, path = item
    plain = run([str(PYFUN), "run", str(path)], cwd=path.parent)
    if plain.returncode != 0:
        return name, "skip", ""  # a starter, a deliberate error, or a typed hole
    out = work / f"native_{abs(hash(name))}"
    build = run([str(PYFUN), "build", "--native", str(path), "-o", str(out)], cwd=path.parent)
    if build.returncode != 0:
        errors = [l for l in (build.stdout + build.stderr).splitlines() if "error" in l]
        return name, "build failed", "\n".join(errors[:5])
    native = run([sys.executable, str(out)], cwd=path.parent)
    shutil.rmtree(out, ignore_errors=True)
    if native.returncode != 0:
        return name, "native run failed", native.stderr.strip()[-400:]
    if native.stdout != plain.stdout:
        diff = [
            f"line {n + 1}: plain {p!r}, native {q!r}"
            for n, (p, q) in enumerate(zip(plain.stdout.splitlines(), native.stdout.splitlines()))
            if p != q
        ] or ["output lengths differ"]
        return name, "differs", "\n".join(diff[:5])
    return name, "ok", ""


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("-j", type=int, default=os.cpu_count() or 2)
    args = ap.parse_args()
    if not PYFUN.exists():
        sys.exit(f"no compiler at {PYFUN}; run `cargo build` (or set PYFUN_BIN)")
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        items = list(corpus(work))
        with ThreadPoolExecutor(args.j) as pool:
            results = list(pool.map(lambda item: compare(item, work), items))
    failures = known = 0
    for name, status, detail in results:
        if status == "skip":
            continue
        if name in KNOWN:
            if status == "ok":
                print(f"[FAIL] {name}: passes now, so take it out of KNOWN")
                failures += 1
            else:
                print(f"[KNOWN] {name}: {status} ({KNOWN[name]})")
                for line in detail.splitlines():
                    print(f"    {line}")
                known += 1
            continue
        print(f"[{'OK ' if status == 'ok' else 'FAIL'}] {name}" + ("" if status == "ok" else f": {status}"))
        for line in detail.splitlines():
            print(f"    {line}")
        failures += status != "ok"
    compared = sum(status != "skip" for _, status, _ in results)
    print(f"{compared - failures - known} of {compared} programs match natively, {known} known failures")
    sys.exit(1 if failures else 0)


main()
