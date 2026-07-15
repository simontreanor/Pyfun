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

// A lazy runner over pyodide-worker.js. Preferred channel: a SharedWorker, so ONE
// compiler + Python runtime serves every page on the site (the first run anywhere
// pays the load; every later page connects to the live instance). Browsers without
// module SharedWorkers (e.g. Firefox) fall back to a per-page dedicated worker.
//
// run(code)      -> Promise<{ out, err }>       execute emitted Python
// compile(src)   -> Promise<{ ok, python, diagnostics }>   compile Pyfun in the worker
export function createRunner(base) {
  let seq = 0;
  let post = null;
  const pending = new Map(); // id -> resolve
  const inflight = new Map(); // id -> message (replayed if shared mode fails)

  function onMessage(event) {
    const resolve = pending.get(event.data.id);
    if (resolve) {
      pending.delete(event.data.id);
      inflight.delete(event.data.id);
      resolve(event.data);
    }
  }

  function useDedicated(url) {
    const w = new Worker(url, { type: "module" });
    w.onmessage = onMessage;
    w.onerror = (e) => {
      for (const resolve of pending.values()) {
        resolve({ out: "", err: "worker failed to start: " + (e.message || e) });
      }
      pending.clear();
      inflight.clear();
    };
    post = (m) => w.postMessage(m);
  }

  function ensureChannel() {
    if (post) return;
    const url = new URL("./pyodide-worker.js", base);
    if (typeof SharedWorker !== "undefined") {
      try {
        const sw = new SharedWorker(url, { type: "module", name: "pyfun-runner" });
        sw.port.onmessage = onMessage;
        sw.onerror = () => {
          // The script failed in shared mode (typically: module SharedWorkers
          // unsupported). Rebuild as a per-page worker and replay what was queued.
          useDedicated(url);
          for (const msg of inflight.values()) post(msg);
        };
        sw.port.start();
        post = (m) => sw.port.postMessage(m);
        return;
      } catch {
        // Constructor refused outright; use the dedicated path below.
      }
    }
    useDedicated(url);
  }

  function send(msg) {
    ensureChannel();
    inflight.set(msg.id, msg);
    return new Promise((resolve) => {
      pending.set(msg.id, resolve);
      post(msg);
    });
  }

  return {
    loaded: () => post !== null,
    run: (code) => send({ id: ++seq, code }).then(({ out, err }) => ({ out, err })),
    compile: (source) =>
      send({ id: ++seq, source }).then(({ compiled, err }) => {
        if (!compiled) throw new Error(err || "compiler failed to load");
        return compiled;
      }),
    // Boot the compiler and Python runtime without running anything, so a page can
    // warm the worker in the background before the user clicks Run.
    warm: () => send({ id: ++seq, warm: true }),
  };
}

// The localStorage flag marking that this visitor has used Run before; pages check it
// to decide whether background-warming the runtime is worth the download/CPU.
export const WARM_FLAG = "pyfun-runner-used";

export function markRunnerUsed() {
  try {
    localStorage.setItem(WARM_FLAG, "1");
  } catch {
    // Storage can be unavailable (privacy modes); warming is just an optimization.
  }
}

export function runnerWasUsed() {
  try {
    return localStorage.getItem(WARM_FLAG) === "1";
  } catch {
    return false;
  }
}
