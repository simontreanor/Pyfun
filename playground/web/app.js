// The playground front end: load the WASM compiler, recompile (debounced) on every
// edit, render the emitted Python + diagnostics, and — on demand — run that Python in
// CPython-via-WebAssembly (Pyodide). `compile` returns a JSON string produced by
// playground/src/lib.rs.
import init, { compile } from "./pkg/pyfun_playground.js";

const DEFAULT_SOURCE = `type Shape = Circle float | Rect float float

let area s =
  match s:
    case Circle r: 3.14159 * r * r
    case Rect w h: w * h

# A List is a Python list; a record is a plain (frozen) dataclass.
let shapes = [Circle 2.0, Rect 3.0 4.0]
let total = List.fold (fun acc s -> acc + area s) 0.0 shapes

print (f"total area: {total}")

# Try it: delete the \`Rect\` case above and watch the exhaustiveness error appear,
# or hit Run to execute the compiled Python right here in the browser.
`;

const editor = document.getElementById("editor");
const output = document.getElementById("output");
const diagnostics = document.getElementById("diagnostics");
const runBtn = document.getElementById("run");
const runOutput = document.getElementById("run-output");

// The last successfully compiled Python (what Run executes), or null when the program
// doesn't compile.
let lastPython = null;

// Byte offset -> 1-based line/column. Spans are byte offsets; for the ASCII the
// examples use this matches character indices. (Non-ASCII would need a byte walk.)
function lineCol(source, offset) {
  let line = 1;
  let col = 1;
  const end = Math.min(offset, source.length);
  for (let i = 0; i < end; i++) {
    if (source[i] === "\n") {
      line++;
      col = 1;
    } else {
      col++;
    }
  }
  return { line, col };
}

function render() {
  const source = editor.value;
  let result;
  try {
    result = JSON.parse(compile(source));
  } catch (err) {
    output.textContent = "internal error: " + err;
    return;
  }

  diagnostics.innerHTML = "";
  for (const d of result.diagnostics) {
    const { line, col } = lineCol(source, d.start);
    const el = document.createElement("div");
    el.className = "diag diag-" + d.severity;
    el.textContent = `L${line}:${col}  ${d.message}`;
    diagnostics.appendChild(el);
  }
  diagnostics.classList.toggle("has-diags", result.diagnostics.length > 0);

  if (result.ok) {
    output.textContent = result.python;
    output.classList.remove("output-empty");
    lastPython = result.python;
  } else {
    output.textContent = result.diagnostics.length
      ? "# fix the problem(s) below to see the compiled Python"
      : "# (nothing to compile yet)";
    output.classList.add("output-empty");
    lastPython = null;
  }
  // Run is available only when there is Python to run.
  runBtn.disabled = lastPython === null;
}

let timer = null;
editor.addEventListener("input", () => {
  clearTimeout(timer);
  timer = setTimeout(render, 150);
});

// --- Pyodide (CPython in the browser), loaded lazily on the first Run ---

const PYODIDE_URL = "https://cdn.jsdelivr.net/pyodide/v314.0.2/full/";
let pyodidePromise = null;

function ensurePyodide() {
  if (!pyodidePromise) {
    pyodidePromise = globalThis.loadPyodide({ indexURL: PYODIDE_URL });
  }
  return pyodidePromise;
}

// Run `code` and return { out, err }: captured stdout+stderr, and the Python traceback
// if it raised. Each run uses a fresh namespace so module state can't leak between runs;
// stdout is redirected to a StringIO (version-proof, no Pyodide-specific stream API).
async function runPython(code) {
  const pyodide = await ensurePyodide();
  pyodide.runPython(`
import sys, io
__pf_buf = io.StringIO()
__pf_saved = (sys.stdout, sys.stderr)
sys.stdout = sys.stderr = __pf_buf
`);
  let err = null;
  try {
    await pyodide.runPythonAsync(code, { globals: pyodide.toPy({}) });
  } catch (e) {
    err = String(e.message || e);
  } finally {
    pyodide.runPython("sys.stdout, sys.stderr = __pf_saved");
  }
  const out = pyodide.runPython("__pf_buf.getvalue()");
  return { out, err };
}

runBtn.addEventListener("click", async () => {
  if (lastPython === null) return;
  const code = lastPython;
  runOutput.hidden = false;
  runOutput.classList.remove("run-error");
  runOutput.textContent = pyodidePromise
    ? "running…"
    : "loading Python runtime… (first run downloads ~10 MB, then it's cached)";
  runBtn.disabled = true;
  try {
    const { out, err } = await runPython(code);
    if (err) {
      runOutput.textContent = (out ? out + "\n" : "") + err;
      runOutput.classList.add("run-error");
    } else {
      runOutput.textContent = out.length ? out : "(ran with no output)";
    }
  } catch (e) {
    runOutput.textContent = "runtime error: " + e;
    runOutput.classList.add("run-error");
  } finally {
    runBtn.disabled = lastPython === null;
  }
});

async function main() {
  await init();
  editor.value = DEFAULT_SOURCE;
  render();
}

main();
