// The site's execution engine: the Pyfun compiler (WASM) and Pyodide (CPython-in-WASM)
// run here, off the main thread, so the UI stays responsive while the ~10 MB Python
// runtime loads and while code executes.
//
// This script serves two worker modes with one body:
//   - As a SharedWorker (the preferred path, see pyfun-run.js): ONE instance serves
//     every page on the site. The first Run anywhere loads the runtime; every other
//     lesson page and the playground then reuse it, so nothing reloads per page.
//   - As a dedicated Worker (the fallback for browsers without module SharedWorkers,
//     e.g. Firefox): per-page, the original behaviour.
//
// Protocol, per port: { id, code } runs Python and answers { id, out, err };
// { id, source } compiles Pyfun and answers { id, compiled } (the object produced by
// playground/src/lib.rs: { ok, python, diagnostics }).

// A module worker: import Pyodide's ES-module build directly (cross-origin `import` is
// CORS-clean on jsdelivr; classic-worker `importScripts` of the CDN is blocked).
import { loadPyodide } from "https://cdn.jsdelivr.net/pyodide/v314.0.2/full/pyodide.mjs";
import initCompiler, { compile } from "./pkg/pyfun_playground.js";

const PYODIDE_URL = "https://cdn.jsdelivr.net/pyodide/v314.0.2/full/";

let pyodideReady = null;
function ensurePyodide() {
  if (!pyodideReady) {
    pyodideReady = loadPyodide({ indexURL: PYODIDE_URL });
  }
  return pyodideReady;
}

let compilerReady = null;
function ensureCompiler() {
  if (!compilerReady) {
    compilerReady = initCompiler();
  }
  return compilerReady;
}

async function execute(code) {
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
  return { out, err };
}

// Runs are serialized: the stdout/stderr redirect is interpreter-global state, and in
// shared mode two pages can post at the same moment.
let queue = Promise.resolve();

function handleWith(post) {
  return async (event) => {
    const { id, code, source, warm } = event.data;
    if (warm) {
      // Pre-boot both engines (sent at page load for returning users, so the
      // runtime is ready by the time they click Run).
      try {
        await Promise.all([ensureCompiler(), ensurePyodide()]);
        post({ id, ready: true });
      } catch (e) {
        post({ id, ready: false, err: String((e && e.message) || e) });
      }
      return;
    }
    if (source !== undefined) {
      // Compilation is pure and reentrant; no need to join the run queue.
      try {
        await ensureCompiler();
        post({ id, compiled: JSON.parse(compile(source)) });
      } catch (e) {
        post({ id, compiled: null, err: "compiler error: " + String((e && e.message) || e) });
      }
      return;
    }
    queue = queue
      .then(() => execute(code))
      .then(
        ({ out, err }) => post({ id, out, err }),
        (e) => post({ id, out: "", err: "runtime error: " + String((e && e.message) || e) })
      );
  };
}

if (typeof SharedWorkerGlobalScope !== "undefined" && self instanceof SharedWorkerGlobalScope) {
  self.onconnect = (event) => {
    const port = event.ports[0];
    port.onmessage = handleWith((msg) => port.postMessage(msg));
  };
} else {
  self.onmessage = handleWith((msg) => self.postMessage(msg));
}
