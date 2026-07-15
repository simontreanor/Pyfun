// Shared compile-and-run plumbing for every surface that embeds the compiler:
// the playground page (app.js) and the docs site's runnable code blocks
// (docs/theme/docs-run.js dynamically imports this module from /playground/).
//
// `base` is a URL the playground's files resolve against (for app.js that is
// import.meta.url; for the docs widget it is the site's playground/ directory),
// so both callers share one copy of the WASM and the worker script.

// Load the WASM compiler and return a function source -> parsed result object
// ({ ok, python, diagnostics }), the JSON produced by playground/src/lib.rs.
export async function loadCompiler(base) {
  const mod = await import(new URL("./pkg/pyfun_playground.js", base).href);
  await mod.default();
  return (source) => JSON.parse(mod.compile(source));
}

// A lazy Pyodide runner. The worker (and Pyodide's ~10 MB runtime) loads on the
// first run() and is reused after; run() resolves with { out, err }.
export function createRunner(base) {
  let worker = null;
  let seq = 0;
  const pending = new Map();

  function ensureWorker() {
    if (!worker) {
      worker = new Worker(new URL("./pyodide-worker.js", base), { type: "module" });
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

  return {
    loaded: () => worker !== null,
    run(code) {
      const w = ensureWorker();
      const id = ++seq;
      return new Promise((resolve) => {
        pending.set(id, resolve);
        w.postMessage({ id, code });
      });
    },
  };
}
