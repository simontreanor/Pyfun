"""Independently verify the learner-track lessons.

For every docs/src/learn/NN-*.md file:
  1. every playground deep link decodes to EXACTLY the pyfun code block that
     precedes it (the exercise starter, as displayed);
  2. the solution block (first ```pyfun after <summary>Show solution</summary>)
     compiles and runs, and its stdout equals the "Expected output:" console block;
  3. every other ```pyfun block in the file at least type-checks (`pyfun check`),
     except blocks whose preceding text marks them as deliberately failing
     (containing a typed hole `?` or introduced as a broken starter).

Usage: python verify_lessons.py
Exit code 0 = all good; prints a per-file report either way.
"""

import base64
import re
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(r"C:\git\Pyfun")
LEARN = REPO / "docs" / "src" / "learn"
PYFUN = REPO / "target" / "debug" / "pyfun.exe"
LINK_RE = re.compile(r"\]\(https://simontreanor\.github\.io/Pyfun/playground/#code=([A-Za-z0-9_-]+)\)")
BLOCK_RE = re.compile(r"```pyfun\n(.*?)```", re.DOTALL)


def decode(enc: str) -> str:
    pad = "=" * (-len(enc) % 4)
    return base64.urlsafe_b64decode(enc + pad).decode("utf-8")


def run_pyfun(cmd: str, code: str) -> subprocess.CompletedProcess:
    with tempfile.NamedTemporaryFile(
        "w", suffix=".pyfun", delete=False, encoding="utf-8", newline="\n"
    ) as f:
        f.write(code)
        path = f.name
    return subprocess.run(
        [str(PYFUN), cmd, path], capture_output=True, text=True, timeout=120
    )


def norm(s: str) -> str:
    return "\n".join(line.rstrip() for line in s.replace("\r\n", "\n").strip().split("\n"))


failures = 0
for md in sorted(LEARN.glob("[0-9][0-9]-*.md")):
    text = md.read_text(encoding="utf-8")
    problems = []

    # 1. Deep links match the preceding pyfun block.
    for m in LINK_RE.finditer(text):
        linked = decode(m.group(1))
        before = text[: m.start()]
        blocks = BLOCK_RE.findall(before)
        if not blocks:
            problems.append("deep link with no preceding pyfun block")
            continue
        starter = blocks[-1]
        if norm(linked) != norm(starter):
            problems.append(
                f"deep link decodes to different code than the displayed starter "
                f"(link {len(linked)} chars vs block {len(starter)} chars)"
            )

    # 2. Solution runs and matches expected output.
    sol_m = re.search(
        r"<summary>Show solution</summary>.*?```pyfun\n(.*?)```", text, re.DOTALL
    )
    exp_m = re.search(r"Expected output:\s*\n+```console\n(.*?)```", text, re.DOTALL)
    if sol_m and exp_m:
        proc = run_pyfun("run", sol_m.group(1))
        if proc.returncode != 0:
            problems.append(f"solution failed to run: {proc.stderr.strip()[:300]}")
        elif norm(proc.stdout) != norm(exp_m.group(1)):
            problems.append(
                "solution output != expected output\n"
                f"      got:      {norm(proc.stdout)[:200]!r}\n"
                f"      expected: {norm(exp_m.group(1))[:200]!r}"
            )
    elif not sol_m:
        problems.append("no solution block found")
    elif not exp_m:
        problems.append("no 'Expected output:' console block found")

    # 3. Other pyfun blocks at least check (unless deliberately failing).
    for i, block in enumerate(BLOCK_RE.findall(text)):
        if sol_m and norm(block) == norm(sol_m.group(1)):
            continue
        deliberate = "?" in re.sub(r'"[^"]*"', "", block)  # holes outside strings
        if re.search(r"^import [A-Z]", block, re.MULTILINE):
            continue  # file-based module example: cannot compile as a lone temp file
        proc = run_pyfun("check", block)
        if proc.returncode != 0 and not deliberate:
            pos = text.find(block)
            # A deliberate-error demonstration quotes its diagnostic in a console
            # block right after the code. Recognize that, and hold the lesson to
            # it: the quoted first error line must match the real one.
            after = text[pos + len(block) : pos + len(block) + 600]
            quoted = re.search(r"```console\n(error:[^\n]*)", after)
            if quoted:
                if quoted.group(1).strip() not in proc.stderr:
                    problems.append(
                        f"block {i}: quoted diagnostic differs from the real one\n"
                        f"      quoted: {quoted.group(1).strip()[:150]!r}\n"
                        f"      real:   {proc.stderr.strip().splitlines()[0][:150]!r}"
                    )
                continue
            # Starters are allowed to fail; only flag blocks BEFORE the Exercise
            # heading (worked examples must be clean).
            ex_pos = text.find("## Exercise")
            if ex_pos == -1 or pos < ex_pos:
                problems.append(
                    f"worked-example block {i} fails check: {proc.stderr.strip()[:200]}"
                )

    status = "OK " if not problems else "FAIL"
    print(f"[{status}] {md.name}")
    for p in problems:
        print(f"    - {p}")
        failures += 1

sys.exit(1 if failures else 0)
