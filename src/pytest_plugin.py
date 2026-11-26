"""`pyfun test`: pytest over a project's `.pyfun` test files.

`pyfun test` compiles each test file with the modules it imports, stages the
emitted Python in a temp directory with this file beside it, and runs this file
with the test files' own paths as its arguments. pytest is then told that a
`.pyfun` test file is a module, the staged one, so a test is known by its
source (`test_shapes.pyfun::test_rect`), and a failure is reported at the
source line with the source statement, from the same tables a traceback of
`pyfun run` is read with (`_pyfun_trace`). The values in a failed assertion are
pytest's own account and are left as they are.

`_pyfun_tests.json` here says which source file is which staged module, and
names every staged module, so that an `assert` in any of them is explained.
"""

import importlib
import json
import os
import sys

import pytest

import _pyfun_trace as trace

_MANIFEST = "_pyfun_tests.json"
_CONFIG = "_pyfun_pytest.ini"
# What pytest opens a line of the failure's explanation with.
_FAIL = "E   "


def _key(path):
    return os.path.normcase(os.path.abspath(path))


def _lookup(here):
    """The tables of the files staged in `here`, as `_pyfun_trace` reads them.
    A staged module that has no table is shown as Python has it."""
    staged = trace._staged_lookup(here)

    def lookup(filename):
        table = staged(filename)
        if table is trace.HIDE and not os.path.basename(filename).startswith("_pyfun_"):
            return None
        return table

    return lookup


def _place(lookup, path, lineno):
    """Where an emitted line came from: `(source path, line, statement)`,
    `trace.HIDE` for a line the compiler wrote, or None for a line of a file
    that is not a staged one."""
    table = lookup(os.path.abspath(path))
    if table is None or table is trace.HIDE:
        return table
    row = trace._row(table["rows"], lineno)
    if row is None or not row[1] or not row[2]:
        return trace.HIDE
    return (row[3] if len(row) > 3 else table["source"], row[1], row[2])


def _explanation(lines):
    """The lines of an entry that explain the failure, set at the margin."""
    for index, line in enumerate(lines):
        if line.startswith(_FAIL):
            margin = len(line) - len(line[1:].lstrip())
            return [
                _FAIL + each[margin:] if not each[1:margin].strip() else each
                for each in lines[index:]
            ]
    return []


def _statement(statement, style):
    """A source statement as an entry shows it."""
    first = (">   " if style == "long" else "    ") + statement[0].strip()
    return [first] + ["    " + line for line in trace._rest(statement)]


def _entries(traceback, lookup, from_source):
    """The entries of a traceback, each of an emitted file shown as its
    source. `from_source` leaves out what stands above the first of those:
    the machinery that imported a test file."""
    shown = []
    in_source = not from_source
    # The explanation of a failure raised in a line the compiler wrote, and
    # the message that went with it: they belong to the entry above.
    carried = None
    for entry in traceback.reprentries:
        where = getattr(entry, "reprfileloc", None)
        found = None if where is None else _place(lookup, where.path, where.lineno)
        if found is trace.HIDE:
            carried = (_explanation(entry.lines), where.message)
            continue
        carried = None
        if found is None:
            if in_source:
                shown.append(entry)
            continue
        in_source = True
        source, line, statement = found
        entry.lines = _statement(statement, entry.style) + _explanation(entry.lines)
        where.path, where.lineno = source, line
        # A lambda's frame sits on its statement's line: the statement is
        # shown once, by the frame that goes furthest into it.
        above = getattr(shown[-1], "reprfileloc", None) if shown else None
        if above is not None and (above.path, above.lineno) == (source, line):
            shown.pop()
        if where.message.startswith("in _pf_"):
            where.message = "in <fun>"
        shown.append(entry)
    if carried is not None and shown and hasattr(shown[-1], "reprfileloc"):
        last = shown[-1]
        last.lines = list(last.lines) + carried[0]
        if last.style == "long" and last.reprfileloc is not None:
            last.reprfileloc.message = carried[1]
    return shown


def _nearest(excinfo, lookup):
    """The innermost source line of an exception's traceback, if it has one."""
    for entry in reversed(excinfo.traceback):
        found = _place(lookup, str(entry.path), entry.lineno + 1)
        if isinstance(found, tuple):
            return found
    return None


def _in_source(shown, excinfo, lookup, from_source=False):
    """pytest's account of a failure, with each line of an emitted file given
    as the source line it came from."""
    traceback = getattr(shown, "reprtraceback", None)
    if traceback is None:
        return shown
    if traceback.style == "native":
        # Python's own traceback, which the hook of `pyfun run` rewrites.
        text = trace.render(excinfo.type, excinfo.value, excinfo.tb, lookup)
        entry = traceback.reprentries[0]
        entry.lines = text.splitlines(keepends=True)
    chain = getattr(shown, "chain", None) or [(traceback, shown.reprcrash, None)]
    for each, crash, _ in chain:
        if each.style != "native":
            entries = _entries(each, lookup, from_source)
            if entries:
                each.reprentries = entries
        if crash is None:
            continue
        found = _place(lookup, crash.path, crash.lineno)
        if found is trace.HIDE:
            # Raised in a line the compiler wrote: the failure is placed at
            # the nearest source line that led there.
            last = each.reprentries[-1] if each.reprentries else None
            where = getattr(last, "reprfileloc", None)
            if where is not None:
                found = (where.path, where.lineno)
            elif crash is shown.reprcrash:
                found = _nearest(excinfo, lookup)
        if isinstance(found, tuple):
            crash.path, crash.lineno = found[0], found[1]
    return shown


class SourceModule(pytest.Module):
    """A `.pyfun` test file: the module is the staged Python it compiled to."""

    staged = None
    lookup = None

    def _getobj(self):
        return importlib.import_module(self.staged)

    def repr_failure(self, excinfo):
        shown = super().repr_failure(excinfo)
        return _in_source(shown, excinfo, self.lookup, from_source=True)


class SourceFunction(pytest.Function):
    """A test of a `.pyfun` file, placed and reported in its source."""

    def reportinfo(self):
        path, line, name = super().reportinfo()
        found = _place(self.parent.lookup, str(path), (line or 0) + 1)
        if isinstance(found, tuple):
            return os.path.abspath(found[0]), found[1] - 1, name
        return path, line, name

    def repr_failure(self, excinfo):
        shown = super().repr_failure(excinfo)
        return _in_source(shown, excinfo, self.parent.lookup)


class _Plugin:
    def __init__(self, here, tests, modules):
        self.tests = tests
        self.modules = modules
        self.lookup = _lookup(here)
        self.here = here

    # Ahead of Hypothesis's own plugin, which loads a profile named with
    # `--hypothesis-profile` over the one set here.
    @pytest.hookimpl(tryfirst=True)
    def pytest_configure(self):
        # An `assert` in a module the tests import is explained like one in a
        # test. pytest's import hook is in place by now, and no staged module
        # has been imported yet.
        pytest.register_assert_rewrite(*self.modules)
        _hypothesis_profile(self.here)

    def pytest_collect_file(self, file_path, parent):
        staged = self.tests.get(_key(str(file_path)))
        if staged is None:
            return None
        module = SourceModule.from_parent(parent, path=file_path)
        module.staged = staged
        module.lookup = self.lookup
        return module

    @pytest.hookimpl(tryfirst=True)
    def pytest_pycollect_makeitem(self, collector, name, obj):
        if not isinstance(collector, SourceModule):
            return None
        # A test is a function the file itself defines whose name opens with
        # `test_`; nothing else in the module is one.
        if (
            name.startswith("test_")
            and callable(obj)
            and getattr(obj, "__module__", None) == collector.staged
        ):
            return SourceFunction.from_parent(collector, name=name)
        return []

    @pytest.hookimpl(wrapper=True)
    def pytest_report_header(self):
        # The configuration is the empty one staged with the tests, which
        # there is no use in naming.
        lines = yield
        return [
            line
            for group in lines
            for line in ([group] if isinstance(group, str) else group)
            if not line.startswith("configfile:")
        ]


def _root(sources):
    """The folder test ids are relative to: the working directory, or the
    nearest folder that holds it and every test file."""
    here = os.getcwd()
    try:
        return os.path.commonpath([here, *sources])
    except ValueError:
        return here


def _hypothesis_profile(here):
    """How Hypothesis runs a property test, where it is there to run one.
    What it keeps on disk (its caches, a `.hypothesis` folder in the working
    directory unless told otherwise) is kept with the staged files, so that
    nothing is left in the project; there is no example database, since a
    failure's example would not outlive the run; and there is no deadline, so
    that a slow machine does not fail a test that is right.
    `--hypothesis-profile` after `--` chooses another profile. Hypothesis is
    imported only once pytest has started, so that pytest's import hook is
    the one that loads it."""
    try:
        from hypothesis import settings
    except ImportError:
        return
    from hypothesis.configuration import set_hypothesis_home_dir

    set_hypothesis_home_dir(os.path.join(here, ".hypothesis"))
    settings.register_profile("pyfun", database=None, deadline=None)
    settings.load_profile("pyfun")


def main(argv):
    here = os.path.dirname(os.path.abspath(__file__))
    with open(os.path.join(here, _MANIFEST), encoding="utf-8") as f:
        manifest = json.load(f)
    tests = {_key(source): module for source, module in manifest["tests"]}
    options = [
        # Nothing is left behind in the project, and nothing of it is read as
        # pytest's configuration.
        "-p",
        "no:cacheprovider",
        "--noconftest",
        "-c",
        os.path.join(here, _CONFIG),
        "--rootdir",
        _root(list(tests)),
    ]
    sys.argv[0] = "pytest"
    return pytest.main(options + argv, plugins=[_Plugin(here, tests, manifest["modules"])])


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
