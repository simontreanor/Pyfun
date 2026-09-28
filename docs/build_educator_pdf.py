"""Print the educator pack as one PDF, for departments that circulate paper.

Run after `mdbook build docs`. It gathers the pack's rendered pages (the overview,
the five session plans, and the answer keys) into one print page beside them,
reusing the book's own stylesheets and code highlighting, and has headless
Chrome print that page to docs/book/educators/educator-pack.pdf. Links stay live
in the PDF, so each playground link still opens its exercise.

Usage:
  python docs/build_educator_pdf.py [BOOK_DIR]

BOOK_DIR defaults to docs/book. Chrome is found on PATH (google-chrome, chromium,
chrome) or at its usual Windows and macOS install paths; set CHROME to override.
"""

import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
SITE = "https://simontreanor.github.io/Pyfun/"
PAGES = [
    "index.html",
    "session-1.html",
    "session-2.html",
    "session-3.html",
    "session-4.html",
    "session-5.html",
    "answer-keys.html",
]
# Scripts the print page keeps: code highlighting, and the book script that
# applies it. Search, clipboard and run buttons have no place on paper.
KEEP_SCRIPTS = ("highlight", "book")


def find_chrome() -> str:
    if os.environ.get("CHROME"):
        return os.environ["CHROME"]
    for name in ("google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "chrome"):
        found = shutil.which(name)
        if found:
            return found
    for path in (
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    ):
        if os.path.exists(path):
            return path
    sys.exit("no Chrome or Chromium found; set CHROME to its path")


def main_of(html: str, page: str) -> str:
    m = re.search(r"<main>(.*?)</main>", html, re.DOTALL)
    if not m:
        sys.exit(f"{page}: no <main> element; was it built by mdBook?")
    return m.group(1)


def absolute_links(body: str) -> str:
    """A printed page travels without the site, so each relative link in it
    points at the published page instead."""

    def fix(m):
        target = m.group(1)
        if re.match(r"[a-z]+:|#", target):
            return m.group(0)
        if target.startswith("../"):
            return f'href="{SITE}{target[3:]}"'
        return f'href="{SITE}educators/{target}"'

    return re.sub(r'href="([^"]*)"', fix, body)


def print_page(educators: Path) -> str:
    first = (educators / PAGES[0]).read_text(encoding="utf-8")
    head = re.search(r"<head>(.*?)</head>", first, re.DOTALL).group(1)
    head = re.sub(r"<title>.*?</title>", "<title>Pyfun educator pack</title>", head, flags=re.DOTALL)
    # The book loads a highlight stylesheet per theme and lets its script switch
    # the dark ones off; paper is always the light theme, so drop them here.
    head = re.sub(
        r'<link rel="stylesheet" id="mdbook-(?:tomorrow-night|ayu-highlight)-css"[^>]*>', "", head
    )
    scripts = [
        tag
        for tag in re.findall(r'<script src="[^"]+"></script>', first)
        if any(f"/{name}-" in tag for name in KEEP_SCRIPTS)
    ]
    sections = []
    for i, page in enumerate(PAGES):
        body = absolute_links(main_of((educators / page).read_text(encoding="utf-8"), page))
        brk = ' style="break-before: page"' if i else ""
        sections.append(f"<section{brk}>{body}</section>")
    return (
        '<!DOCTYPE HTML>\n<html lang="en" class="light" dir="ltr">\n<head>'
        + head
        + "<style>@page { margin: 18mm 16mm; } main { max-width: none; }"
        " .buttons, .nav-chapters, .mobile-nav-chapters { display: none !important; }</style>"
        + '</head>\n<body>\n<div id="mdbook-content" class="content"><main>\n'
        + "\n".join(sections)
        + "\n</main></div>\n"
        + "\n".join(scripts)
        + "\n</body>\n</html>\n"
    )


def main() -> int:
    book = Path(sys.argv[1]) if len(sys.argv) > 1 else REPO / "docs" / "book"
    educators = book.resolve() / "educators"
    html_path = educators / "educator-pack-print.html"
    pdf_path = educators / "educator-pack.pdf"
    html_path.write_text(print_page(educators), encoding="utf-8")
    subprocess.run(
        [
            find_chrome(),
            "--headless=new",
            "--disable-gpu",
            "--no-sandbox",
            "--no-pdf-header-footer",
            "--virtual-time-budget=10000",
            f"--print-to-pdf={pdf_path}",
            html_path.as_uri(),
        ],
        check=True,
        capture_output=True,
    )
    html_path.unlink()
    if not pdf_path.exists() or pdf_path.stat().st_size < 10_000:
        sys.exit(f"Chrome did not produce {pdf_path}")
    print(f"wrote {pdf_path} ({pdf_path.stat().st_size // 1024} KiB)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
