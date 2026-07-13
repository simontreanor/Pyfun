"""The Pyfun Jupyter kernel: ipykernel wrapper over `pyfun kernel-engine`.

Division of labour (mirrors the REPL, `src/repl.rs` in the Pyfun repo, with the
roles inverted): the Rust engine owns the *session* — accumulated definitions,
type checking, inferred-type echoes, and the diff of emitted top-level Python
chunks — while this process owns the *namespace* and simply ``exec``s the new
chunks the engine hands back. Because the kernel is itself the Python runtime,
cell output, tracebacks, ``input()``, and KeyboardInterrupt need no plumbing.

Protocol frames are 8 ASCII digits (payload byte length) + UTF-8 payload.
Request: frame(op), frame(payload) with op in {"eval", "type"}.
Response: frame("ok"|"error"), frame(message), frame(python-blob).
"""

import os
import re
import shutil
import subprocess
import sys
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
        unusable even after a respawn. Successful "eval" payloads are logged for
        replay.
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
        if op == "eval" and status == "ok":
            self._log.append(payload)
        return status, message, blob

    def _respawn_and_replay(self):
        self.close()
        self._spawn()
        for cell in self._log:
            # Rebuild the session state; blobs are discarded (already executed).
            self._roundtrip("eval", cell)

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
        if not code.strip():
            return {
                "status": "ok",
                "execution_count": self.execution_count,
                "payload": [],
                "user_expressions": {},
            }
        try:
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
            try:
                exec(blob, self._ns)
            except BaseException:
                text = traceback.format_exc()
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
