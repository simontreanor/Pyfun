// The bundle's loader (`pyfun bundle`, `DESIGN.md` §6 "Browser target"): boots
// Pyodide from its CDN, stages the compiled Python files and any assets into the
// in-browser file system, and runs the entry module. The page's DOM is reachable
// from the program through Pyodide's `js` module (`extern import js`), stdout and
// stderr land in the two `<pre>` elements, and a raised exception prints there
// too. A module script, so top-level `await` is available.
import { loadPyodide } from "__PYODIDE_URL__pyodide.mjs";

const FILES = __FILES__;
const ASSETS = __ASSETS__;
const ENTRY = "__ENTRY__";

const stdout = document.getElementById("pyfun-stdout");
const stderr = document.getElementById("pyfun-stderr");
const status = document.getElementById("pyfun-status");
const append = (el, text) => {
  el.textContent += text;
};

status.textContent = "loading Python…";
const pyodide = await loadPyodide({
  indexURL: "__PYODIDE_URL__",
  stdout: (line) => append(stdout, line + "\n"),
  stderr: (line) => append(stderr, line + "\n"),
});

// The compiled modules import each other as siblings (`import geometry`), so
// they are staged in the working directory, which is first on `sys.path`.
for (const name of FILES) {
  const source = await (await fetch(new URL(name, import.meta.url))).text();
  pyodide.FS.writeFile(name, source);
}
for (const name of ASSETS) {
  const bytes = new Uint8Array(await (await fetch(new URL(name, import.meta.url))).arrayBuffer());
  pyodide.FS.writeFile(name, bytes);
}
pyodide.runPython("import sys\nif '' not in sys.path: sys.path.insert(0, '')");

status.textContent = "";
try {
  const entry = pyodide.FS.readFile(ENTRY, { encoding: "utf8" });
  await pyodide.runPythonAsync(entry);
} catch (e) {
  append(stderr, String((e && e.message) || e) + "\n");
}
