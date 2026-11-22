"""The Pyfun Jupyter kernel: ipykernel wrapper over `pyfun kernel-engine`.

Division of labour (mirrors the REPL, `src/repl.rs` in the Pyfun repo, with the
roles inverted): the Rust engine owns the *session* — accumulated definitions,
type checking, inferred-type echoes, and the diff of emitted top-level Python
chunks — while this process owns the *namespace* and simply ``exec``s the new
chunks the engine hands back. Because the kernel is itself the Python runtime,
cell output, tracebacks, ``input()``, and KeyboardInterrupt need no plumbing.

Protocol frames are 8 ASCII digits (payload byte length) + UTF-8 payload.
Request: frame(op), frame(payload) with op in {"eval", "eval-in", "type", "hook"}.
Response: frame("ok"|"error"), frame(message), frame(python-blob).

A cell that fails when it runs is reported in the lines the author wrote. The
engine supplies both halves: "hook" returns the source of the module that
rewrites a traceback, and "eval-in" (payload: the cell's name, a newline, the
cell) answers with a blob whose first line is its table, saying which statement
of which cell each emitted line came from. An engine that predates either op
answers "error", and the kernel then runs cells with "eval" and shows Python's
own traceback.
"""

import json
import os
import re
import shutil
import subprocess
import sys
import threading
import traceback

from ipykernel.kernelbase import Kernel

try:
    from importlib.metadata import version as _dist_version

    _VERSION = _dist_version("pyfun-lang")
except Exception:  # pragma: no cover - source checkouts, odd installs
    _VERSION = "unknown"


class EngineError(RuntimeError):
    """The engine process is unavailable or the protocol broke."""


class Engine:
    """One `pyfun kernel-engine` subprocess plus the cell log for replay.

    The engine holds the accumulated definitions; if it ever dies (a compiler
    panic, an OOM kill), a fresh one is spawned and every previously successful
    cell is re-sent to rebuild that state. The returned blobs are discarded
    during replay — this namespace already executed them — so replay has no
    runtime side effects.
    """

    def __init__(self):
        self._proc = None
        self._log = []

    @staticmethod
    def _binary():
        exe = os.environ.get("PYFUN_BIN")
        if exe:
            return exe
        # Prefer the binary installed alongside this interpreter (the wheel
        # puts it in Scripts/bin of the same environment) — a stale `pyfun`
        # elsewhere on PATH may predate the kernel-engine protocol.
        scripts = "Scripts" if os.name == "nt" else "bin"
        suffix = ".exe" if os.name == "nt" else ""
        local = os.path.join(sys.exec_prefix, scripts, "pyfun" + suffix)
        if os.path.exists(local):
            return local
        exe = shutil.which("pyfun")
        if not exe:
            raise EngineError(
                "cannot find the `pyfun` binary in this environment or on PATH "
                "(pip install pyfun-lang); set PYFUN_BIN to override"
            )
        return exe

    def _spawn(self):
        self._proc = subprocess.Popen(
            [self._binary(), "kernel-engine"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            # stderr inherited: an engine crash surfaces in the kernel log.
        )

    def _roundtrip(self, op, payload):
        proc = self._proc
        try:
            for part in (op, payload):
                data = part.encode("utf-8")
                proc.stdin.write(b"%08d" % len(data))
                proc.stdin.write(data)
            proc.stdin.flush()
            frames = []
            for _ in range(3):
                header = proc.stdout.read(8)
                if len(header) < 8:
                    raise EngineError("engine closed the protocol stream")
                length = int(header)
                body = proc.stdout.read(length)
                if len(body) < length:
                    raise EngineError("engine closed the protocol stream")
                frames.append(body.decode("utf-8"))
            return frames
        except (OSError, ValueError) as exc:
            raise EngineError(f"engine protocol failure: {exc}") from exc

    def request(self, op, payload):
        """One protocol round-trip; respawns and replays the log on a dead engine.

        Returns (status, message, blob); raises EngineError when the engine is
        unusable even after a respawn. Successful "eval" and "eval-in" requests
        are logged for replay.
        """
        replayed = False
        if self._proc is None or self._proc.poll() is not None:
            self._respawn_and_replay()
            replayed = self._log != []
        try:
            status, message, blob = self._roundtrip(op, payload)
        except EngineError:
            if replayed:
                raise  # a freshly rebuilt engine failed too — give up
            self._respawn_and_replay()
            status, message, blob = self._roundtrip(op, payload)
        if op in ("eval", "eval-in") and status == "ok":
            self._log.append((op, payload))
        return status, message, blob

    def _respawn_and_replay(self):
        self.close()
        self._spawn()
        for op, cell in self._log:
            # Rebuild the session state; blobs are discarded (already executed).
            self._roundtrip(op, cell)

    def reset(self):
        """Forget every definition: the next request spawns a fresh engine with
        nothing to replay."""
        self.close()
        self._log = []

    def close(self):
        if self._proc is not None:
            try:
                self._proc.kill()
                self._proc.wait()
            except OSError:
                pass
            self._proc = None


class _StreamProxy:
    """A sys.stdout/sys.stderr stand-in that forwards writes as Jupyter stream
    messages *synchronously*, so cell output lands inside its own execution
    window (ipykernel's fd-level capture forwards asynchronously, which can
    slip past the end of a short cell). `silent` swallows the output."""

    def __init__(self, kernel, name, silent):
        self._kernel = kernel
        self._name = name
        self._silent = silent

    def write(self, text):
        if text and not self._silent:
            self._kernel._stream(self._name, text)
        return len(text)

    def flush(self):
        pass

    def isatty(self):
        return False

    @property
    def encoding(self):
        return "utf-8"


class PyfunKernel(Kernel):
    implementation = "pyfun"
    implementation_version = _VERSION
    language = "pyfun"
    language_version = _VERSION
    language_info = {
        "name": "pyfun",
        "mimetype": "text/x-pyfun",
        "file_extension": ".pyfun",
        "pygments_lexer": "fsharp",  # closest existing lexer
        "codemirror_mode": "mllike",
    }
    banner = (
        "Pyfun — an F#-inspired, functional-first language that compiles to "
        "readable Python.\nDefinitions echo their inferred types; state "
        "persists across cells."
    )

    def __init__(self, **kwargs):
        super().__init__(**kwargs)
        self._engine = Engine()
        self._ns = {}
        # The traceback hook's namespace: None until asked for, False when the
        # engine has none to give.
        self._hook = None
        # The table of each blob that came with one, by the name it ran under.
        self._tables = {}
        self._blobs = 0

    def _traceback_hook(self):
        """The engine's traceback hook as a namespace, or False without one."""
        if self._hook is None:
            self._hook = False
            try:
                status, source, _blob = self._engine.request("hook", "")
                if status == "ok":
                    hook = {"__name__": "pyfun_traceback"}
                    exec(compile(source, "<pyfun traceback hook>", "exec"), hook)
                    self._hook = hook
            except Exception:
                pass
        return self._hook

    def _lookup(self, filename):
        """What the hook asks of each frame: a blob's table, or to leave out
        this module's own frames."""
        if filename in self._tables:
            return self._tables[filename]
        if filename == __file__:
            return self._hook["HIDE"]
        return None

    def _format_error(self):
        """The traceback of the exception being handled, in the cell's lines
        when the engine said where the emitted lines came from."""
        if self._hook:
            try:
                return self._hook["render"](*sys.exc_info(), self._lookup)
            except Exception:
                pass
        return traceback.format_exc()

    def _stream(self, name, text):
        self.send_response(self.iopub_socket, "stream", {"name": name, "text": text})

    def _error_reply(self, ename, text):
        return {
            "status": "error",
            "execution_count": self.execution_count,
            "ename": ename,
            "evalue": text.splitlines()[0] if text else ename,
            "traceback": text.splitlines(),
        }

    def do_execute(
        self, code, silent, store_history=True, user_expressions=None, allow_stdin=False
    ):
        # `:reset` on a cell's first line starts a fresh session, as it does in
        # the REPL: the engine forgets every definition and the namespace is
        # emptied, then the rest of the cell runs. A notebook of standalone
        # examples (the lesson notebooks) uses it where one example would
        # otherwise redefine another's types.
        first, _, rest = code.lstrip().partition("\n")
        if first.strip() == ":reset":
            self._engine.reset()
            self._ns = {}
            self._tables = {}
            code = rest
        if not code.strip():
            return {
                "status": "ok",
                "execution_count": self.execution_count,
                "payload": [],
                "user_expressions": {},
            }
        table = ""
        try:
            status = "error"
            if self._traceback_hook():
                # The cell under the name a traceback gives it.
                named = f"In[{self.execution_count}]\n{code}"
                status, message, blob = self._engine.request("eval-in", named)
                if status == "ok":
                    table, _, blob = blob.partition("\n")
                elif message.startswith("unknown op"):
                    self._hook = False
            if not self._hook:
                status, message, blob = self._engine.request("eval", code)
        except EngineError as exc:
            text = f"pyfun engine error: {exc}"
            if not silent:
                self._stream("stderr", text + "\n")
            return self._error_reply("EngineError", text)

        if status != "ok":
            if not silent:
                self._stream("stderr", message)
            return self._error_reply("PyfunError", message)

        if message and not silent:
            self._stream("stdout", message)
        if blob:
            saved = sys.stdout, sys.stderr
            sys.stdout = _StreamProxy(self, "stdout", silent)
            sys.stderr = _StreamProxy(self, "stderr", silent)
            self._blobs += 1
            name = f"<pyfun-{self._blobs}>"
            if table:
                try:
                    self._tables[name] = json.loads(table)
                except ValueError:
                    pass
            try:
                self._exec(blob, name)
            except BaseException:
                text = self._format_error()
                if not silent:
                    self._stream("stderr", text)
                return self._error_reply("PythonError", text)
            finally:
                sys.stdout, sys.stderr = saved
        return {
            "status": "ok",
            "execution_count": self.execution_count,
            "payload": [],
            "user_expressions": {},
        }

    def _exec(self, blob, name):
        """Run a cell's Python in this namespace, compiled under `name` so a
        traceback's frames can be told apart cell by cell.

        ipykernel runs cells inside its own event loop, and `asyncio.run` (how a
        Pyfun program runs an `async { }` block) refuses to start while a loop
        is running in its thread. A cell that uses asyncio therefore runs on a
        worker thread, which has no loop of its own, and the kernel waits for
        it; any other cell runs here, where an interrupt reaches it directly.
        """
        code = compile(blob, name, "exec")
        if "asyncio" not in blob:
            exec(code, self._ns)
            return
        outcome = {}

        def work():
            try:
                exec(code, self._ns)
            except BaseException as exc:  # re-raised on the kernel's thread
                outcome["error"] = exc

        worker = threading.Thread(target=work, name="pyfun-cell", daemon=True)
        worker.start()
        while worker.is_alive():
            worker.join(0.05)
        if "error" in outcome:
            raise outcome["error"]

    def do_inspect(self, code, cursor_pos, detail_level=0, omit_sections=()):
        """Shift-Tab: the inferred type of the identifier under the cursor."""
        target = _token_at(code, cursor_pos)
        found = False
        data = {}
        if target:
            try:
                status, message, _blob = self._engine.request("type", target)
                if status == "ok":
                    found = True
                    data = {"text/plain": message}
            except EngineError:
                pass
        return {
            "status": "ok",
            "found": found,
            "data": data,
            "metadata": {},
        }

    def do_shutdown(self, restart):
        self._engine.close()
        return {"status": "ok", "restart": restart}


_TOKEN = re.compile(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*")


def _token_at(code, cursor_pos):
    """The (possibly dotted) identifier spanning cursor_pos, or None."""
    for match in _TOKEN.finditer(code):
        if match.start() <= cursor_pos <= match.end():
            return match.group(0)
    return None
