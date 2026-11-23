"""Check `pyfun compile --typed` with a real type checker.

Three things are checked, with mypy:

1. A consumer of a typed module is held to its types. A small program is
   compiled with `--typed` and a Python script uses it; mypy must accept every
   correct use and reject every wrong one, at the line that makes it.
2. A typed project is clean. `examples/modules` is compiled with `--typed -o`;
   the directory must hold `py.typed`, pass mypy with nothing to report, and
   print what the untyped project prints.
3. No annotation is malformed. Every lesson program and single-file example
   is compiled with `--typed`; each must parse as Python, and mypy must find
   nothing wrong with an annotation itself (a name it cannot resolve, a type
   that is not one, a wrong number of type arguments). What mypy says about
   the insides of emitted function bodies is not checked here: those are the
   compiler's own idioms, and a consumer of the package never sees them.

Usage: python tests/typed_output.py
Needs `pip install mypy`. The compiler it runs is `target/debug/pyfun`
(override with PYFUN_BIN), and the interpreter that has mypy is the one running
this script (override with PYFUN_MYPY_PYTHON).
"""

import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
LEARN = REPO / "docs" / "src" / "learn"
PYFUN = Path(
    os.environ.get("PYFUN_BIN")
    or next(
        (p for p in (REPO / "target/debug/pyfun.exe", REPO / "target/debug/pyfun") if p.exists()),
        REPO / "target/debug/pyfun",
    )
)
MYPY_PYTHON = os.environ.get("PYFUN_MYPY_PYTHON") or sys.executable
BLOCK_RE = re.compile(r"```pyfun\n(.*?)```", re.DOTALL)
# mypy's codes for an annotation that is wrong in itself.
ANNOTATION_CODES = {"name-defined", "valid-type", "type-arg", "type-var", "syntax", "misc"}

failures = []


def check(label, ok, evidence=""):
    print(f"[{'PASS' if ok else 'FAIL'}] {label}" + (f"\n{evidence}" if evidence and not ok else ""))
    if not ok:
        failures.append(label)


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


def mypy(cwd, *targets):
    """mypy's error lines for `targets`, checked from `cwd`."""
    done = run(
        [MYPY_PYTHON, "-m", "mypy", "--python-version", "3.12", "--no-error-summary",
         "--show-error-codes", "--no-incremental", "--ignore-missing-imports", *targets],
        cwd=cwd,
    )
    return [line for line in done.stdout.splitlines() if ": error:" in line]


SHAPES = '''\
type Shape =
    | Circle(float)
    | Rect { width: float, height: float }

type Box a = Box(a)

type Point = { x: int, y: int }

## The area of a shape.
let area shape =
    match shape:
        case Circle(r): 3.14 * r * r
        case Rect { width, height }: width * height

let double x = x * 2

let pair x y = (x, y)

let apply f x = f x

let origin = Point { x = 0, y = 0 }

let first xs = List.head xs

let lookup key table = Map.find key table
'''

# Each line marked `# E` must draw an error from mypy, and no other line may.
CONSUMER = '''\
from shapes import Box, Circle, Point, Rect, Shape, Some, apply, area, double, first, lookup, origin, pair

a: float = area(Circle(1.0))
b: float = area(Rect(width=2.0, height=3.0))
c: int = double(2)
d: float = double(2.5)
p: tuple[int, str] = pair(1, "x")
n: int = apply(len, "abc")
o: Point = origin
f = first([1, 2, 3])
if isinstance(f, Some):
    v: int = f._0
k = lookup("a", {"a": 1.5})
bx: Box[int] = Box(3)
s: Shape = Circle(2.0)

area("circle")  # E
bad: str = double(2)  # E
pair(1)  # E
w: Box[str] = Box(3)  # E
x: int = double(2.5)  # E
'''


def consumer_is_held_to_the_types(work: Path):
    (work / "shapes.pyfun").write_text(SHAPES, encoding="utf-8")
    done = run([str(PYFUN), "compile", "--typed", "shapes.pyfun", "-o", "shapes.py"], cwd=work)
    check("a typed module compiles", done.returncode == 0, done.stderr)
    (work / "consumer.py").write_text(CONSUMER, encoding="utf-8")
    expected = {
        i + 1 for i, line in enumerate(CONSUMER.splitlines()) if line.rstrip().endswith("# E")
    }
    errors = mypy(work, "consumer.py")
    found = {int(line.split(":")[1]) for line in errors if line.startswith("consumer.py:")}
    check(
        "mypy rejects each wrong use of a typed module, and only those",
        found == expected,
        f"expected errors on lines {sorted(expected)}, found {sorted(found)}:\n" + "\n".join(errors),
    )
    module_errors = mypy(work, "shapes.py")
    check("the typed module itself is clean", not module_errors, "\n".join(module_errors))


def typed_project_is_clean(work: Path):
    entry = REPO / "examples" / "modules" / "main.pyfun"
    out = work / "typed"
    done = run([str(PYFUN), "compile", "--typed", str(entry), "-o", str(out)])
    check("a typed project compiles", done.returncode == 0, done.stderr)
    check("a typed project carries py.typed", (out / "py.typed").is_file())
    errors = mypy(out, ".")
    check("a typed project passes mypy", not errors, "\n".join(errors))
    plain = work / "plain"
    run([str(PYFUN), "compile", str(entry), "-o", str(plain)])
    typed_out = run([sys.executable, "main.py"], cwd=out)
    plain_out = run([sys.executable, "main.py"], cwd=plain)
    check(
        "a typed project prints what the untyped one prints",
        typed_out.returncode == 0 and typed_out.stdout == plain_out.stdout,
        typed_out.stderr,
    )


def corpus():
    """(name, source) for every lesson program and single-file example."""
    for md in sorted(LEARN.glob("[0-9][0-9]-*.md")):
        text = md.read_text(encoding="utf-8").replace("\r\n", "\n")
        for i, block in enumerate(BLOCK_RE.findall(text)):
            yield f"lesson_{md.stem[:2]}_{i}", block
    for path in sorted((REPO / "examples").rglob("*.pyfun")):
        text = path.read_text(encoding="utf-8")
        if re.search(r"^(?:import|from) [A-Z]", text, re.MULTILINE):
            continue
        name = "_".join(path.relative_to(REPO / "examples").with_suffix("").parts)
        yield "ex_" + re.sub(r"\W", "_", name), text


def no_annotation_is_malformed(work: Path):
    out = work / "corpus"
    out.mkdir()
    compiled = []
    for name, source in corpus():
        src = out / f"{name}.pyfun"
        src.write_text(source, encoding="utf-8")
        done = run([str(PYFUN), "compile", "--typed", src.name, "-o", f"{name}.py"], cwd=out)
        src.unlink()
        if done.returncode == 0:
            compiled.append(out / f"{name}.py")
    check(f"the corpus compiles with --typed ({len(compiled)} programs)", len(compiled) > 80)
    unparsed = []
    for path in compiled:
        try:
            compile(path.read_text(encoding="utf-8"), str(path), "exec")
        except SyntaxError as exc:
            unparsed.append(f"{path.name}: {exc}")
    check("every typed program parses as Python", not unparsed, "\n".join(unparsed))
    malformed = [
        line
        for line in mypy(out, ".")
        if (m := re.search(r"\[([a-z-]+)\]$", line)) and m.group(1) in ANNOTATION_CODES
    ]
    check("mypy finds no malformed annotation", not malformed, "\n".join(malformed[:20]))


def main():
    probe = run([MYPY_PYTHON, "-m", "mypy", "--version"])
    if probe.returncode != 0:
        print("mypy is not installed for", MYPY_PYTHON, "(pip install mypy)")
        return 2
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        consumer_is_held_to_the_types(work)
        typed_project_is_clean(work)
        no_annotation_is_malformed(work)
    if failures:
        print(f"=== {len(failures)} FAILED ===")
        return 1
    print("=== ALL PASS ===")
    return 0


if __name__ == "__main__":
    sys.exit(main())
