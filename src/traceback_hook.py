"""An error's traceback, reported in `.pyfun` lines.

The compiler pairs the Python it emits with a table saying which source
statement each emitted line came from. With that table a traceback is rewritten:
a frame of the emitted code is shown as its source file, line and statement; a
frame the compiler wrote (a prelude helper, the shared runtime, the host's own
machinery) is left out; a library's frame stays as Python shows it.

Three hosts use this module, and each brings its own `lookup`: `pyfun run`
stages it beside the emitted files and calls `run`; the REPL's worker and the
Jupyter kernel execute its source and call `render`.

A table is `{"source": name, "rows": [[emitted line, source line, [statement
lines], label], ...]}`, rows in emitted-line order. An emitted line belongs to
the nearest row at or above it. A row whose source line is 0 says the lines
under it come from no source; a row's label, when it has one, names the source
in place of the table's.
"""

import os
import sys

# The answer of a `lookup` for a frame the compiler or the host wrote.
HIDE = object()
# The most lines of one statement a frame shows.
_SHOWN = 6


def _row(rows, emitted_line):
    """The row an emitted line belongs to: the nearest at or above it."""
    found = None
    for row in rows:
        if row[0] > emitted_line:
            break
        found = row
    return found


def _rest(statement):
    """The lines of a statement after its first, indented as the source has
    them relative to that first line."""
    head = statement[0]
    margin = len(head) - len(head.lstrip())
    rest = [
        line[margin:] if not line[:margin].strip() else line.lstrip()
        for line in statement[1:]
    ]
    while rest and not rest[-1].strip():
        rest.pop()
    if len(rest) >= _SHOWN:
        rest = rest[: _SHOWN - 2]
        last = rest[-1]
        rest.append(last[: len(last) - len(last.lstrip())] + "...")
    return rest


def _kinds(traceback):
    """A frame that carries the rest of its statement, and the stack that prints it."""

    class SourceFrame(traceback.FrameSummary):
        rest = ()

    class SourceStack(traceback.StackSummary):
        def format_frame_summary(self, frame, **kwargs):
            shown = super().format_frame_summary(frame, **kwargs)
            return shown + "".join("    " + line + "\n" for line in getattr(frame, "rest", ()))

    return SourceFrame, SourceStack


def _translate(frame, lookup, source_frame):
    """The frame as the source has it, or None for a frame to leave out."""
    table = lookup(frame.filename)
    if table is None:
        return frame
    if table is HIDE:
        return None
    row = _row(table["rows"], frame.lineno or 0)
    if row is None or not row[1] or not row[2]:
        return None
    statement = row[2]
    source = row[3] if len(row) > 3 else table["source"]
    name = "<fun>" if frame.name.startswith("_pf_") else frame.name
    shown = source_frame(source, row[1], name, lookup_line=False, line=statement[0].strip())
    shown.rest = _rest(statement)
    return shown


def _rewrite(error, lookup, kinds, seen):
    if error is None or id(error) in seen:
        return
    seen.add(id(error))
    source_frame, source_stack = kinds
    stack = source_stack()
    for frame in error.stack:
        shown = _translate(frame, lookup, source_frame)
        if shown is None:
            continue
        # A lambda's frame sits on its statement's line: the statement was just shown.
        if stack and (stack[-1].filename, stack[-1].lineno) == (shown.filename, shown.lineno):
            shown.rest = ()
        stack.append(shown)
    error.stack = stack
    _rewrite(error.__cause__, lookup, kinds, seen)
    _rewrite(error.__context__, lookup, kinds, seen)
    for inner in getattr(error, "exceptions", None) or ():
        _rewrite(inner, lookup, kinds, seen)


def _colour(stream):
    """The keyword that asks for a coloured traceback, where Python has one."""
    try:
        import _colorize

        return {"colorize": _colorize.can_colorize(file=stream)}
    except Exception:
        return {}


def render(kind, error, tb, lookup, stream=None):
    """The traceback text for an exception, in `.pyfun` lines.

    `lookup(filename)` answers for each frame: the table of an emitted file,
    `HIDE` for a frame to leave out, or None for a frame to show as it is.
    `stream` is where the text will be written, which decides its colour.
    """
    import traceback

    described = traceback.TracebackException(kind, error, tb)
    _rewrite(described, lookup, _kinds(traceback), set())
    if stream is not None:
        try:
            return "".join(described.format(**_colour(stream)))
        except TypeError:
            pass
    return "".join(described.format())


def _staged_lookup(here):
    """The `lookup` of `pyfun run`: the files staged in the directory `here`,
    with their tables in `_pyfun_lines.json` there."""
    here = os.path.normcase(os.path.realpath(here))
    tables = []
    staged = {}

    def lookup(filename):
        directory = os.path.dirname(filename)
        known = staged.get(directory)
        if known is None:
            known = os.path.normcase(os.path.realpath(directory)) == here
            staged[directory] = known
        if not known:
            return None
        if not tables:
            import json

            with open(os.path.join(here, "_pyfun_lines.json"), encoding="utf-8") as f:
                tables.append(json.load(f))
        return tables[0].get(os.path.basename(filename), HIDE)

    return lookup


def run(entry):
    """Run the staged program `entry` as `__main__`, as `python entry` would,
    reporting an uncaught error in `.pyfun` lines."""
    import __main__

    here = os.path.dirname(os.path.abspath(__file__))
    lookup = _staged_lookup(here)

    def excepthook(kind, error, tb):
        try:
            text = render(kind, error, tb, lookup, sys.stderr)
        except Exception:
            sys.__excepthook__(kind, error, tb)
            return
        sys.stderr.write(text)
        sys.stderr.flush()

    path = os.path.join(here, entry)
    with open(path, "rb") as f:
        code = compile(f.read(), path, "exec", dont_inherit=True)
    sys.excepthook = excepthook
    sys.argv[0] = path
    scope = __main__.__dict__
    scope.pop("_pyfun_trace", None)
    scope["__file__"] = path
    exec(code, scope)
