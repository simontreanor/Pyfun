# Pyfun Playground (WebAssembly)

The real Pyfun compiler, built to WebAssembly, compiling Pyfun to readable Python
**live in the browser** as you type. It's a separate crate so the core `pyfun`
compiler stays dependency-free — only this shim (`src/lib.rs`) depends on
`wasm-bindgen`, and it just calls the same pure `pyfun::analyze` / `pyfun::compile`
entry points the CLI and LSP use, so the Python shown is byte-for-byte the CLI's output.

## Build

Prerequisites (the Rust toolchain is pinned by `rust-toolchain.toml`):

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-pack
```

From the repo root, build the WASM package into `playground/web/pkg/`:

```bash
wasm-pack build playground --target web --out-dir web/pkg --release
```

WASM ES modules don't load over `file://`, so serve the folder over HTTP:

```bash
python -m http.server -d playground/web 8000
# open http://localhost:8000
```

`wasm-pack` runs natively on Windows — no WSL needed to build or serve.

## Deploy (GitHub Pages)

[`.github/workflows/pages.yml`](../.github/workflows/pages.yml) builds the WASM **and**
the mdBook docs site (`docs/`) into one Pages artifact on every push to `main`: the docs
land at the site root and this playground at **`/playground/`**
(<https://simontreanor.github.io/Pyfun/playground/>). Enable it once under
**Settings → Pages → Source: GitHub Actions**.

## Permalinks

The URL hash carries a program, so lessons and posts can link to exact code:

- `#code=<url-safe base64 of the UTF-8 source>` loads that program into the editor
  (the **Copy link** button writes one for the current editor contents);
- `#example=<n>` loads the nth curated example.

A malformed hash is ignored and the default example loads instead.

## Recording the demo GIF from it

The playground is a normal web page, so record it natively on any OS — no `ttyd`/WSL:

- **Quick:** [ScreenToGif](https://www.screentogif.com/) (Windows), Kap (macOS), or Peek
  (Linux) — capture the two panes as you type.
- **Reproducible:** a Playwright script that types into `#editor` and records video, then
  `ffmpeg` to GIF — the browser equivalent of a VHS tape, and it runs on Windows.

## What it does

- **Compiles live.** Parse, type/effect/exhaustiveness-check, and lower to Python —
  everything the compiler does up to emitting source, with live diagnostics (the
  resilient `analyze`), as you type.
- **Runs it.** The **Run** button executes the emitted Python in **CPython itself**,
  compiled to WebAssembly ([Pyodide](https://pyodide.org)), and shows stdout. Pyodide runs
  in a worker (`pyodide-worker.js`), off the main thread, so loading the ~10 MB runtime
  (lazy, on first Run) and executing code never freeze the UI. The worker is a
  **SharedWorker** where supported: one compiler + Python runtime serves this page AND
  every runnable code block on the docs site, so nothing reloads per page (browsers
  without module SharedWorkers fall back to a per-page worker; runs are serialized inside
  the worker since pages share one interpreter). Each run uses a fresh namespace and
  captures stdout/stderr via a `StringIO` redirect; a Python exception shows its traceback. Programs that only touch the stdlib (`json`, `sqlite3`,
  `math`, `statistics`, `dataclasses`, …) run as-is; an `extern` for a third-party package
  (`numpy`, `requests`) would need `micropip` (not wired up) and network calls don't work in
  the sandbox.
