// Progressive enhancement: give every ```pyfun code block a Run button that
// compiles the block with the real WASM compiler and executes the emitted Python
// via the playground's Pyodide worker, right on the page. The block becomes
// editable, so exercises can be solved in place.
//
// Everything here is additive: if this script (or the WASM, or the worker) fails
// to load, the page is exactly the static, highlighted book it was without it.
(function () {
  "use strict";

  // mdBook defines `path_to_root` inline on every page; fall back to this
  // script's own URL (it is emitted at <root>/theme/docs-run-<hash>.js).
  var root;
  if (typeof path_to_root !== "undefined") {
    root = path_to_root;
  } else if (document.currentScript && document.currentScript.src) {
    root = document.currentScript.src.replace(/theme\/[^/]*$/, "");
  } else {
    return;
  }
  var playgroundBase = new URL(root + "playground/", location.href).href;

  var compilerPromise = null;
  var runner = null;

  function plumbing() {
    if (!compilerPromise) {
      compilerPromise = import(playgroundBase + "pyfun-run.js").then(function (mod) {
        runner = mod.createRunner(playgroundBase);
        return mod.loadCompiler(playgroundBase);
      });
    }
    return compilerPromise;
  }

  function attach(code) {
    var pre = code.parentElement;
    if (!pre || pre.tagName !== "PRE") return;

    var bar = document.createElement("div");
    bar.className = "pyfun-run-bar";
    var btn = document.createElement("button");
    btn.type = "button";
    btn.className = "pyfun-run-btn";
    btn.textContent = "▶ Run";
    btn.title = "Compile this block and run the emitted Python, right here";
    var hint = document.createElement("span");
    hint.className = "pyfun-run-hint";
    hint.textContent = "the block is editable";
    bar.appendChild(btn);
    bar.appendChild(hint);

    var out = document.createElement("pre");
    out.className = "pyfun-run-out";
    out.hidden = true;

    pre.insertAdjacentElement("afterend", out);
    pre.insertAdjacentElement("afterend", bar);

    // Editable exercises: plaintext-only where supported, plain contenteditable
    // elsewhere. textContent stays the source of truth either way.
    try {
      code.contentEditable = "plaintext-only";
    } catch (e) {
      code.contentEditable = "true";
    }
    code.spellcheck = false;

    btn.addEventListener("click", function () {
      out.hidden = false;
      out.className = "pyfun-run-out";
      out.textContent = runner && runner.loaded()
        ? "running…"
        : "loading the compiler and Python runtime… (first run on a page downloads ~10 MB, then it is cached)";
      btn.disabled = true;
      plumbing()
        .then(function (compile) {
          var result = compile(code.textContent);
          if (!result.ok) {
            var msgs = result.diagnostics.map(function (d) {
              return d.severity + ": " + d.message;
            });
            out.className = "pyfun-run-out pyfun-run-err";
            out.textContent = msgs.length ? msgs.join("\n") : "(nothing to compile)";
            return null;
          }
          return runner.run(result.python);
        })
        .then(function (res) {
          if (!res) return;
          if (res.err) {
            out.className = "pyfun-run-out pyfun-run-err";
            out.textContent = (res.out ? res.out + "\n" : "") + res.err;
          } else {
            out.textContent = res.out.length ? res.out : "(ran with no output)";
          }
        })
        .catch(function (e) {
          out.className = "pyfun-run-out pyfun-run-err";
          out.textContent = "could not load the runner: " + e;
        })
        .finally(function () {
          btn.disabled = false;
        });
    });
  }

  document.querySelectorAll("code.language-pyfun").forEach(attach);
})();
