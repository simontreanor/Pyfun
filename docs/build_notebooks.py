"""Build the learner-track lessons as Jupyter notebooks.

Each docs/src/learn/NN-*.md lesson becomes docs/src/notebooks/NN-*.ipynb for the
Pyfun kernel (`pip install "pyfun-lang[jupyter]"`, then `python -m pyfun_kernel.install`).
The lesson markdown stays the single source: prose becomes markdown cells, every
```pyfun block outside a solution becomes a code cell, and a solution stays inside
its collapsible <details> so it does not run (or give itself away) by accident. A
cell that redeclares a type or extern from an earlier cell opens with the kernel's
`:reset`, since each lesson example was written as a program of its own.

mdBook copies the notebooks into the site as they are, so each one downloads from
https://simontreanor.github.io/Pyfun/notebooks/<lesson>.ipynb.

Usage:
  python docs/build_notebooks.py          # (re)write every notebook
  python docs/build_notebooks.py --check  # exit 1 if any notebook is out of date
"""

import json
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
LEARN = REPO / "docs" / "src" / "learn"
OUT = REPO / "docs" / "src" / "notebooks"
SITE = "https://simontreanor.github.io/Pyfun/"

FENCE_RE = re.compile(r"^```pyfun\n(.*?)^```\n?", re.DOTALL | re.MULTILINE)
DETAILS_RE = re.compile(r"<details>.*?</details>", re.DOTALL)
LINK_RE = re.compile(r"\]\((?!https?://|#)([^)]+)\)")


def absolute_link(target: str) -> str:
    """A lesson's relative link, made absolute against the published site, since
    a notebook opened on its own has no site around it."""
    path, _, anchor = target.partition("#")
    if path.endswith(".md"):
        path = path[: -len(".md")] + ".html"
    if path.startswith("../"):
        url = SITE + path[len("../") :]
    else:
        url = SITE + "learn/" + path
    return url + ("#" + anchor if anchor else "")


def source_lines(text: str) -> list:
    """Notebook JSON stores a cell's source as a list of lines, each but the last
    keeping its newline."""
    lines = text.splitlines(keepends=True)
    if lines:
        lines[-1] = lines[-1].rstrip("\n")
    return lines


def markdown_cell(text: str) -> dict:
    text = LINK_RE.sub(lambda m: "](" + absolute_link(m.group(1)) + ")", text)
    return {"cell_type": "markdown", "metadata": {}, "source": source_lines(text.strip("\n"))}


def code_cell(code: str) -> dict:
    return {
        "cell_type": "code",
        "execution_count": None,
        "metadata": {},
        "outputs": [],
        "source": source_lines(code.rstrip("\n")),
    }


# A top-level declaration the kernel refuses to repeat within one session.
DECL_RE = re.compile(
    r"^(?:extern\s+type|opaque\s+type|type|extern(?!\s+import)|measure|module)\s+([A-Za-z_]\w*)",
    re.MULTILINE,
)


def with_resets(cells: list) -> list:
    """Lesson examples are standalone programs, while a notebook's cells share
    one session, where a second `type Point` or `extern parse…` is an error. A
    cell that redeclares a name from an earlier cell starts with `:reset`, which
    gives it the fresh session the lesson's example assumed."""
    seen = set()
    for cell in cells:
        if cell["cell_type"] != "code":
            continue
        names = set(DECL_RE.findall("".join(cell["source"])))
        if names & seen:
            cell["source"] = [":reset\n"] + cell["source"]
            seen = set()
        seen |= names
    return cells


def split_cells(markdown: str) -> list:
    """Markdown cells for the prose, code cells for each runnable ```pyfun block.
    A block inside <details> (a solution) is part of the prose around it."""
    protected = [(m.start(), m.end()) for m in DETAILS_RE.finditer(markdown)]

    def in_details(pos: int) -> bool:
        return any(start <= pos < end for start, end in protected)

    cells, cursor = [], 0
    for m in FENCE_RE.finditer(markdown):
        if in_details(m.start()):
            continue
        prose = markdown[cursor : m.start()]
        if prose.strip():
            cells.append(markdown_cell(prose))
        cells.append(code_cell(m.group(1)))
        cursor = m.end()
    rest = markdown[cursor:]
    if rest.strip():
        cells.append(markdown_cell(rest))
    return cells


def notebook(lesson: Path) -> dict:
    web = SITE + "learn/" + lesson.stem + ".html"
    markdown = lesson.read_text(encoding="utf-8")
    title = markdown.splitlines()[0].lstrip("# ").strip()
    intro = markdown_cell(
        f"*This notebook is the lesson [{title}]({web}) from Learn Pyfun, for the Pyfun kernel. "
        "Run the cells from the top: definitions carry forward from cell to cell, and a cell "
        "with a hole `?` shows what belongs there instead of running.*"
    )
    cells = [intro] + with_resets(split_cells(markdown))
    for i, cell in enumerate(cells):
        cell["id"] = f"{lesson.stem[:2]}-{i:03d}"
    return {
        "cells": cells,
        "metadata": {
            "kernelspec": {"display_name": "Pyfun", "language": "pyfun", "name": "pyfun"},
            "language_info": {"file_extension": ".pyfun", "name": "pyfun"},
        },
        "nbformat": 4,
        "nbformat_minor": 5,
    }


def render(nb: dict) -> str:
    return json.dumps(nb, indent=1, ensure_ascii=False) + "\n"


def main() -> int:
    check = "--check" in sys.argv[1:]
    lessons = sorted(LEARN.glob("[0-9][0-9]-*.md"))
    stale = []
    OUT.mkdir(exist_ok=True)
    expected = set()
    for lesson in lessons:
        target = OUT / (lesson.stem + ".ipynb")
        expected.add(target.name)
        text = render(notebook(lesson))
        current = target.read_text(encoding="utf-8") if target.exists() else None
        if current != text:
            stale.append(target.name)
            if not check:
                target.write_text(text, encoding="utf-8", newline="\n")
    extra = sorted(p.name for p in OUT.glob("*.ipynb") if p.name not in expected)
    if check:
        if stale or extra:
            for name in stale:
                print(f"out of date: docs/src/notebooks/{name}")
            for name in extra:
                print(f"no lesson for: docs/src/notebooks/{name}")
            print("run `python docs/build_notebooks.py` and commit the result")
            return 1
        print(f"{len(lessons)} notebooks up to date")
        return 0
    for name in extra:
        (OUT / name).unlink()
    print(f"wrote {len(stale)} of {len(lessons)} notebooks")
    return 0


if __name__ == "__main__":
    sys.exit(main())
