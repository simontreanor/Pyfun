"""An uncaught error, reported in `.pyfun` lines.

`pyfun run` stages this module beside the Python it emitted, with a table
(`_pyfun_lines.json`) saying which source statement each emitted line came
from. When the program ends in an uncaught exception, each frame of the emitted
code is shown as its `.pyfun` file, line and source text; a frame the compiler
wrote (a prelude helper, the shared runtime, this module) is left out, and a
library's frame stays as Python shows it.
"""

import os
import sys

_HERE = os.path.normcase(os.path.realpath(os.path.dirname(os.path.abspath(__file__))))
# The most lines of one statement a frame shows.
_SHOWN = 6
_tables = None
_staged_dirs = {}


def _load():
    global _tables
    if _tables is None:
        import json

        with open(os.path.join(_HERE, "_pyfun_lines.json"), encoding="utf-8") as f:
            _tables = json.load(f)
    return _tables


def _staged(filename):
    """Whether `filename` is one of the files staged beside this module."""
    directory = os.path.dirname(filename)
    known = _staged_dirs.get(directory)
    if known is None:
        known = os.path.normcase(os.path.realpath(directory)) == _HERE
        _staged_dirs[directory] = known
    return known


def _statement(lines, emitted_line):
    """The source lines `(first, last)` an emitted line came from: the nearest
    entry at or above it."""
    found = None
    for emitted, first, last in lines:
        if emitted > emitted_line:
            break
        found = (first, last)
    return found


def _rest(text, first, last):
    """The lines of a statement after its first, indented as the source has them
    relative to that first line."""
    head = text[first - 1]
    margin = len(head) - len(head.lstrip())
    rest = [line[margin:] if not line[:margin].strip() else line.lstrip() for line in text[first:last]]
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


def _translate(frame, source_frame):
    """The frame as the source has it, or None for a frame the compiler wrote."""
    if not _staged(frame.filename):
        return frame
    table = _load().get(os.path.basename(frame.filename))
    if table is None:
        return None
    statement = _statement(table["lines"], frame.lineno or 0)
    if statement is None:
        return None
    first, last = statement
    name = "<fun>" if frame.name.startswith("_pf_") else frame.name
    text = table["text"]
    shown = source_frame(table["source"], first, name, lookup_line=False, line=text[first - 1].strip())
    shown.rest = _rest(text, first, last)
    return shown


def _rewrite(error, kinds, seen):
    if error is None or id(error) in seen:
        return
    seen.add(id(error))
    source_frame, source_stack = kinds
    stack = source_stack()
    for frame in error.stack:
        shown = _translate(frame, source_frame)
        if shown is None:
            continue
        # A lambda's frame sits on its statement's line: the statement was just shown.
        if stack and (stack[-1].filename, stack[-1].lineno) == (shown.filename, shown.lineno):
            shown.rest = ()
        stack.append(shown)
    error.stack = stack
    _rewrite(error.__cause__, kinds, seen)
    _rewrite(error.__context__, kinds, seen)
    for inner in getattr(error, "exceptions", None) or ():
        _rewrite(inner, kinds, seen)


def _colour():
    """The keyword that asks for a coloured traceback, where Python has one."""
    try:
        import _colorize

        return {"colorize": _colorize.can_colorize(file=sys.stderr)}
    except Exception:
        return {}


def render(kind, error, tb):
    """The traceback text for an exception, in `.pyfun` lines."""
    import traceback

    described = traceback.TracebackException(kind, error, tb)
    _rewrite(described, _kinds(traceback), set())
    try:
        return "".join(described.format(**_colour()))
    except TypeError:
        return "".join(described.format())


def _excepthook(kind, error, tb):
    try:
        text = render(kind, error, tb)
    except Exception:
        sys.__excepthook__(kind, error, tb)
        return
    sys.stderr.write(text)
    sys.stderr.flush()


def run(entry):
    """Run the staged program `entry` as `__main__`, as `python entry` would."""
    import __main__

    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), entry)
    with open(path, "rb") as f:
        code = compile(f.read(), path, "exec", dont_inherit=True)
    sys.excepthook = _excepthook
    sys.argv[0] = path
    scope = __main__.__dict__
    scope.pop("_pyfun_trace", None)
    scope["__file__"] = path
    exec(code, scope)
