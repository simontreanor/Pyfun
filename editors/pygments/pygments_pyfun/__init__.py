"""A Pygments lexer for Pyfun (https://github.com/simontreanor/Pyfun).

Installing this package registers the lexer under the alias ``pyfun`` (and the
``*.pyfun`` filename pattern) through Pygments' plugin entry point, so anything
that highlights with Pygments picks it up: Sphinx, MkDocs, nbconvert, and
``pygmentize -l pyfun``.

The token classes follow the Tree-sitter grammar's highlight query
(``editors/tree-sitter-pyfun/queries/highlights.scm``), so Pyfun looks the same
in a rendered page as it does in an editor.
"""

from pygments.lexer import RegexLexer, bygroups, include, words
from pygments.token import (
    Comment,
    Keyword,
    Name,
    Number,
    Operator,
    Punctuation,
    String,
    Text,
    Whitespace,
)

__all__ = ["PyfunLexer"]
__version__ = "0.1.0"

KEYWORDS = (
    "let", "mut", "type", "opaque", "measure", "module", "extern", "fun", "with",
    "as", "import", "if", "then", "elif", "else", "match", "case", "try", "return",
    "yield", "do", "for", "in",
)
OPERATOR_WORDS = ("and", "or", "not")
BUILDERS = ("async", "seq", "result", "option")
BUILTIN_TYPES = ("int", "float", "bool", "string", "unit")
BUILTINS = (
    "print", "input", "abs", "min", "max", "round", "floor", "ceil", "truncate",
    "sqrt", "cbrt", "id", "const", "ignore", "flip", "fst", "snd", "sign",
)


class PyfunLexer(RegexLexer):
    """Pyfun: an F#-inspired, functional-first language that compiles to Python."""

    name = "Pyfun"
    url = "https://github.com/simontreanor/Pyfun"
    aliases = ["pyfun"]
    filenames = ["*.pyfun"]
    mimetypes = ["text/x-pyfun"]
    version_added = ""

    tokens = {
        "root": [
            (r"\s+", Whitespace),
            (r"#.*$", Comment.Single),
            include("strings"),
            include("numbers"),
            # `let!`, `return!`, `yield!`, `do!`: the binding forms of a CE.
            (r"\b(let|return|yield|do)!", Keyword),
            (words(("true", "false"), prefix=r"\b", suffix=r"\b"), Keyword.Constant),
            (words(OPERATOR_WORDS, prefix=r"\b", suffix=r"\b"), Operator.Word),
            (r"\bpure\b", Keyword.Declaration),
            (words(KEYWORDS, prefix=r"\b", suffix=r"\b"), Keyword),
            # A computation-expression builder is a keyword only before `{`.
            (r"\b(" + "|".join(BUILDERS) + r")(\s*)(\{)", bygroups(Keyword.Pseudo, Whitespace, Punctuation)),
            (words(BUILTIN_TYPES, prefix=r"\b", suffix=r"\b"), Keyword.Type),
            (words(BUILTINS, prefix=r"\b", suffix=r"(?![\w.])"), Name.Builtin),
            # A typed hole: `?` or `?name`.
            (r"\?[A-Za-z_]\w*|\?", Name.Builtin.Pseudo),
            # A type variable: `'a`.
            (r"'[A-Za-z_]\w*", Name.Variable.Magic),
            # A qualified member, `List.map` / `Geometry.area`.
            (r"([A-Z]\w*)(\.)([a-z_]\w*)", bygroups(Name.Namespace, Punctuation, Name.Function)),
            # Capitalised names are constructors, types and modules.
            (r"[A-Z]\w*", Name.Class),
            # An effect annotation on an arrow: `->{io, async}`.
            (r"(->)(\{)([^}]*)(\})", bygroups(Operator, Punctuation, Name.Decorator, Punctuation)),
            (r"\|>|<\||>>|<<|->|<-|\*\*|//|==|!=|<=|>=|[-+*/%<>=|:]", Operator),
            (r"_\b", Name.Builtin.Pseudo),
            (r"[a-z_]\w*", Name),
            (r"[{}()\[\],.;]", Punctuation),
            (r".", Text),
        ],
        "numbers": [
            (r"0[xX][0-9a-fA-F_]+", Number.Hex),
            (r"0[oO][0-7_]+", Number.Oct),
            (r"0[bB][01_]+", Number.Bin),
            (r"\d[\d_]*\.\d[\d_]*([eE][+-]?\d+)?|\d[\d_]*[eE][+-]?\d+", Number.Float),
            (r"\d[\d_]*", Number.Integer),
            # A unit of measure right after a number: `9.8<m/s^2>`.
            (r"(?<=\d)<[^<>\n]*>", Name.Decorator),
        ],
        "strings": [
            (r'f"""', String.Affix, "fstring-triple"),
            (r'f"', String.Affix, "fstring"),
            (r'r"""', String, "raw-triple"),
            (r'r"[^"\n]*"', String),
            (r'"""', String, "triple"),
            (r'"', String, "string"),
        ],
        "raw-triple": [
            (r'"""', String, "#pop"),
            (r'[^"]+|"', String),
        ],
        "escapes": [
            (r'\\(["\\nrt0{}]|u\{[0-9a-fA-F]+\})', String.Escape),
        ],
        "string": [
            include("escapes"),
            (r'"', String, "#pop"),
            (r'[^"\\\n]+', String),
            (r"\\", String),
        ],
        "triple": [
            include("escapes"),
            (r'"""', String, "#pop"),
            (r'[^"\\]+|"', String),
        ],
        "fstring": [
            include("escapes"),
            (r"\{\{|\}\}", String.Escape),
            (r"\{", String.Interpol, "interpolation"),
            (r'"', String.Affix, "#pop"),
            (r'[^"\\{}\n]+', String),
        ],
        "fstring-triple": [
            include("escapes"),
            (r"\{\{|\}\}", String.Escape),
            (r"\{", String.Interpol, "interpolation"),
            (r'"""', String.Affix, "#pop"),
            (r'[^"\\{}]+|"', String),
        ],
        "interpolation": [
            (r"\}", String.Interpol, "#pop"),
            (r"=(?=\s*\})", String.Interpol),
            include("root"),
        ],
    }
