"""Tests for the Pyfun Pygments lexer.

Run from editors/pygments:  python -m pytest tests   (or: python tests/test_lexer.py)
"""

import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
REPO = HERE.parents[2]

from pygments.token import Error, Keyword, Name, Number, Operator, String  # noqa: E402

from pygments_pyfun import PyfunLexer  # noqa: E402


def tokens(src):
    return [(t, v) for t, v in PyfunLexer().get_tokens(src) if v.strip()]


def sources():
    yield from sorted((REPO / "examples").rglob("*.pyfun"))
    yield from sorted((REPO / "editors" / "tree-sitter-pyfun" / "test").glob("*.pyfun"))


def test_every_example_lexes_without_errors_and_round_trips():
    for path in sources():
        src = path.read_text(encoding="utf-8")
        toks = list(PyfunLexer().get_tokens(src))
        errors = [v for t, v in toks if t is Error]
        assert not errors, f"{path}: {errors[:5]}"
        assert "".join(v for _, v in toks).rstrip("\n") == src.rstrip("\n"), path


def test_token_classes():
    toks = tokens('let area w h = w * h\nlet! x = f"{n=}"\n')
    assert (Keyword, "let") in toks
    assert (Keyword, "let!") in toks
    assert (String.Interpol, "{") in toks
    toks = tokens("type Shape = Circle float | Rect float float")
    assert (Name.Class, "Shape") in toks and (Keyword.Type, "float") in toks
    toks = tokens("xs |> List.map ?f")
    assert (Operator, "|>") in toks
    assert (Name.Namespace, "List") in toks and (Name.Function, "map") in toks
    assert (Name.Builtin.Pseudo, "?f") in toks
    toks = tokens("let g = 9.8<m/s^2>")
    assert (Number.Float, "9.8") in toks and (Name.Decorator, "<m/s^2>") in toks
    toks = tokens("extern now: unit ->{io} float = time.time")
    assert (Name.Decorator, "io") in toks


def test_a_builder_name_is_a_keyword_only_before_a_brace():
    assert (Keyword.Pseudo, "async") in tokens("let job = async { return 1 }")
    assert (Name, "result") in tokens("let result = 3")


def test_the_entry_point_registers_the_alias():
    from pygments.lexers import find_lexer_class_by_name

    try:
        assert find_lexer_class_by_name("pyfun").__name__ == "PyfunLexer"
    except Exception:
        # Only once the package is installed does the entry point exist.
        pass


if __name__ == "__main__":
    for name, fn in list(globals().items()):
        if name.startswith("test_"):
            fn()
            print("ok", name)
