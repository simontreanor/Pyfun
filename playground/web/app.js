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

// --- Pyodide runs in a Web Worker (pyodide-worker.js), off the main thread, so loading
// the ~10 MB runtime and executing code never freeze the UI. ---

let worker = null;
let runSeq = 0;
const pending = new Map();

function ensureWorker() {
  if (!worker) {
    // A module worker (it `import`s Pyodide's .mjs), resolved relative to this module so
    // it works at any base path (e.g. /Pyfun/).
    worker = new Worker(new URL("./pyodide-worker.js", import.meta.url), { type: "module" });
    worker.onmessage = (event) => {
      const { id, out, err } = event.data;
      const resolve = pending.get(id);
      if (resolve) {
        pending.delete(id);
        resolve({ out, err });
      }
    };
    worker.onerror = (e) => {
      for (const resolve of pending.values()) {
        resolve({ out: "", err: "worker failed to start: " + (e.message || e) });
      }
      pending.clear();
    };
  }
  return worker;
}

// Send `code` to the worker and resolve with { out, err }. The UI disables Run while a
// run is outstanding, so at most one is in flight — but each carries an id anyway.
function runInWorker(code) {
  const w = ensureWorker();
  const id = ++runSeq;
  return new Promise((resolve) => {
    pending.set(id, resolve);
    w.postMessage({ id, code });
  });
}

runBtn.addEventListener("click", async () => {
  if (lastPython === null) return;
  const code = lastPython;
  runOutput.hidden = false;
  runOutput.classList.remove("run-error");
  runOutput.textContent = worker
    ? "running…"
    : "loading Python runtime… (first run downloads ~10 MB, then it's cached)";
  runBtn.disabled = true;
  try {
    const { out, err } = await runInWorker(code);
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
