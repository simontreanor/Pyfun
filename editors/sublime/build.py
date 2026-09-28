"""Build the Sublime Text package's syntax from the shared TextMate grammar.

The VS Code extension's `editors/vscode/pyfun.tmLanguage.json` is the one TextMate
grammar Pyfun keeps (JetBrains bundles it too). Sublime Text reads TextMate
grammars in their property-list form, so this writes the same grammar as
`editors/sublime/Pyfun/Pyfun.tmLanguage`, adding the file extension Sublime uses
to pick the syntax.

Usage:
  python editors/sublime/build.py          # (re)write the syntax
  python editors/sublime/build.py --check  # exit 1 if it is out of date
"""

import json
import plistlib
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SOURCE = REPO / "editors" / "vscode" / "pyfun.tmLanguage.json"
TARGET = REPO / "editors" / "sublime" / "Pyfun" / "Pyfun.tmLanguage"


def render() -> bytes:
    grammar = json.loads(SOURCE.read_text(encoding="utf-8"))
    grammar.pop("$schema", None)
    grammar["fileTypes"] = ["pyfun"]
    grammar["uuid"] = "6c1f3b52-7d3e-4c1a-9a0e-5f2b8d1e4a70"
    return plistlib.dumps(grammar, sort_keys=True)


def main() -> int:
    data = render()
    current = TARGET.read_bytes().replace(b"\r\n", b"\n") if TARGET.exists() else None
    if "--check" in sys.argv[1:]:
        if current != data:
            print(f"{TARGET.relative_to(REPO)} is out of date: run python editors/sublime/build.py")
            return 1
        print("Sublime syntax up to date")
        return 0
    TARGET.write_bytes(data)
    print(f"wrote {TARGET.relative_to(REPO)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
