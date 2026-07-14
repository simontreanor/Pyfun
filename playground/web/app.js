// The playground front end: load the WASM compiler, recompile (debounced) on every
// edit, render the emitted Python + diagnostics, and — on demand — run that Python in
// CPython-via-WebAssembly (Pyodide). `compile` returns a JSON string produced by
// playground/src/lib.rs.
import init, { compile } from "./pkg/pyfun_playground.js";

// Curated examples, shown in the picker above the editor. Every one type-checks AND
// runs in this playground (each was verified against the real compiler); the `# Try it`
// note in each points at a single edit that makes a compiler check fire. The first entry
// is loaded on start. Backticks in the Pyfun comments are escaped so they don't close
// these JS template literals.
const EXAMPLES = [
  {
    label: "Algebraic data types",
    source: `type Shape = Circle float | Rect float float

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
`,
  },
  {
    label: "JSON → typed records",
    source: `# Parse JSON into your own typed records.
# Bad input becomes a value you handle, never a crash.
type User = { name: string, age: int }

# Build a decoder compositionally: \`field\` pulls one key,
# \`map2\` combines the field decoders into the record.
let userDecoder =
  Decode.map2 (fun name age -> User { name = name, age = age })
    (Decode.field "name" Decode.string)
    (Decode.field "age" Decode.int)

# Consuming a result is an exhaustive match on Ok / Error.
let describe r =
  match r:
    case Ok u: f"{u.name} is {u.age}"
    case Error e: f"decode failed: {e.errorMessage}"

let ok      = """{"name": "ada", "age": 36}"""
let missing = """{"name": "bob"}"""
let wrong   = """{"name": "cy", "age": "old"}"""

print (describe (Decode.decodeString userDecoder ok))
print (describe (Decode.decodeString userDecoder missing))
print (describe (Decode.decodeString userDecoder wrong))
`,
  },
  {
    label: "Typed Python FFI",
    source: `# Typed Python FFI: call real libraries with types at the boundary, no runtime library.
# \`extern pure\` names a Python function, gives it a Pyfun type, and promises it is
# side-effect-free; \`pyfun compile\` lowers it to a plain \`import statistics\` + call.
extern pure mean:  List float -> float = statistics.mean
extern pure stdev: List float -> float = statistics.stdev

type Summary = { n: int, mean: float, stdev: float }

let summarize xs =
  Summary { n = List.len xs, mean = mean xs, stdev = stdev xs }

print (summarize [1.0, 2.0, 3.0, 4.0])
`,
  },
  {
    label: "Units of measure",
    source: `# Units of measure: dimensions are checked at compile time, then erased to plain numbers.
measure m
measure s

# Quantities combine by dimension, not just by number. Metres add to metres, and
# dividing a distance by a time gives a derived <m/s> unit, all inferred.
let distance = 100.0<m> + 50.0<m>
let pace = distance / 25.0<s>

print (f"{pace} m/s")

# The check is real: change one 50.0<m> to 50.0<s> and it stops compiling,
# because you cannot add metres to seconds.
`,
  },
  {
    label: "Inferred effects",
    source: `# Effects are inferred, never annotated. Purity propagates through calls, and
# \`let pure\` is a promise the compiler checks: a side effect in the body is an error.
let double x = x * 2

let pure triple x = x * 3

# Uncomment to watch \`let pure\` reject an impure body (print performs io):
# let pure oops x =
#   print x
#   x

print (double 21)
print (triple 14)
`,
  },
];

const editor = document.getElementById("editor");
const output = document.getElementById("output");
const diagnostics = document.getElementById("diagnostics");
const runBtn = document.getElementById("run");
const runOutput = document.getElementById("run-output");
const examplePicker = document.getElementById("examples");

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

// Populate the example picker and load the chosen example into the editor. Selecting an
// example replaces the editor contents (the usual playground behaviour); until then the
// user's own edits are left alone.
for (let i = 0; i < EXAMPLES.length; i++) {
  const opt = document.createElement("option");
  opt.value = String(i);
  opt.textContent = EXAMPLES[i].label;
  examplePicker.appendChild(opt);
}
examplePicker.addEventListener("change", () => {
  const ex = EXAMPLES[Number(examplePicker.value)];
  if (!ex) return;
  editor.value = ex.source;
  render();
  editor.focus();
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
  editor.value = EXAMPLES[0].source;
  render();
}

main();
