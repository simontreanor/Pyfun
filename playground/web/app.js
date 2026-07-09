// The playground front end: load the WASM compiler, then recompile (debounced) on
// every edit and render the emitted Python + diagnostics. `compile` returns a JSON
// string produced by playground/src/lib.rs.
import init, { compile } from "./pkg/pyfun_playground.js";

const DEFAULT_SOURCE = `type Shape = Circle float | Rect float float

let area s =
  match s:
    case Circle r: 3.14159 * r * r
    case Rect w h: w * h

# A List is a Python list; a record is a plain class; effects are inferred.
let shapes = [Circle 2.0, Rect 3.0 4.0]
let total = List.fold (fun acc s -> acc + area s) 0.0 shapes

print total

# Try it: delete the \`Rect\` case above and watch the exhaustiveness error appear.
`;

const editor = document.getElementById("editor");
const output = document.getElementById("output");
const diagnostics = document.getElementById("diagnostics");

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
  } else {
    output.textContent = result.diagnostics.length
      ? "# fix the problem(s) below to see the compiled Python"
      : "# (nothing to compile yet)";
    output.classList.add("output-empty");
  }
}

let timer = null;
editor.addEventListener("input", () => {
  clearTimeout(timer);
  timer = setTimeout(render, 150);
});

async function main() {
  await init();
  editor.value = DEFAULT_SOURCE;
  render();
}

main();
