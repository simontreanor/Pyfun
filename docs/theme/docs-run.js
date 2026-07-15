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

  // Everything heavy (the compiler WASM and the Python runtime) lives in the worker
  // behind createRunner: a SharedWorker where supported, so open tabs share one
  // runtime. A same-tab navigation still restarts it (a SharedWorker dies with its
  // last page), so for visitors who have used Run before, plumbing() is called at
  // page load below and the runtime boots in the background while they read.
  var runnerPromise = null;
  var runner = null;
  var mod = null;

  function plumbing() {
    if (!runnerPromise) {
      runnerPromise = import(playgroundBase + "pyfun-run.js").then(function (m) {
        mod = m;
        runner = m.createRunner(playgroundBase);
        return runner;
      });
    }
    return runnerPromise;
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
        : "starting the compiler and Python runtime… (the first run on the site downloads ~10 MB; every page then shares the same live runtime)";
      btn.disabled = true;
      plumbing()
        .then(function (r) {
          mod.markRunnerUsed();
          return r.compile(code.textContent).then(function (result) {
            if (!result.ok) {
              var msgs = result.diagnostics.map(function (d) {
                return d.severity + ": " + d.message;
              });
              out.className = "pyfun-run-out pyfun-run-err";
              out.textContent = msgs.length ? msgs.join("\n") : "(nothing to compile)";
              return null;
            }
            return r.run(result.python);
          });
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

  var blocks = document.querySelectorAll("code.language-pyfun");
  blocks.forEach(attach);

  // Background warm-up for returning users: if this visitor has clicked Run before,
  // start the worker now so the runtime is ready by the time they click it here.
  if (blocks.length > 0) {
    import(playgroundBase + "pyfun-run.js")
      .then(function (m) {
        if (m.runnerWasUsed()) {
          plumbing().then(function (r) {
            r.warm();
          });
        }
      })
      .catch(function () {
        // No worker, no warm-up; the click path reports real errors.
      });
  }
})();
