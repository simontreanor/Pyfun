// Pyodide (CPython-in-WASM) runs here, in a Web Worker — off the main thread — so the
// UI stays responsive while the ~10 MB runtime loads and while code executes.
//
// Protocol: the page posts { id, code }; this worker runs `code` and posts back
// { id, out, err } — captured stdout/stderr, and a Python traceback if it raised.

// A module worker: import Pyodide's ES-module build directly (cross-origin `import` is
// CORS-clean on jsdelivr; classic-worker `importScripts` of the CDN is blocked).
import { loadPyodide } from "https://cdn.jsdelivr.net/pyodide/v314.0.2/full/pyodide.mjs";

const PYODIDE_URL = "https://cdn.jsdelivr.net/pyodide/v314.0.2/full/";

let pyodideReady = null;
function ensurePyodide() {
  if (!pyodideReady) {
    pyodideReady = loadPyodide({ indexURL: PYODIDE_URL });
  }
  return pyodideReady;
}

self.onmessage = async (event) => {
  const { id, code } = event.data;
  try {
    const pyodide = await ensurePyodide();
    // Redirect stdout/stderr to a StringIO (version-proof, no Pyodide stream API).
    pyodide.runPython(`
import sys, io
__pf_buf = io.StringIO()
__pf_saved = (sys.stdout, sys.stderr)
sys.stdout = sys.stderr = __pf_buf
`);
    let err = null;
    try {
      // A fresh namespace each run, so module state can't leak between runs.
      await pyodide.runPythonAsync(code, { globals: pyodide.toPy({}) });
    } catch (e) {
      err = String((e && e.message) || e);
    } finally {
      pyodide.runPython("sys.stdout, sys.stderr = __pf_saved");
    }
    const out = pyodide.runPython("__pf_buf.getvalue()");
    self.postMessage({ id, out, err });
  } catch (e) {
    // Failure to load Pyodide, or an internal error — report it as the run error.
    self.postMessage({ id, out: "", err: "runtime error: " + String((e && e.message) || e) });
  }
};
