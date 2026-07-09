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

[`.github/workflows/playground.yml`](../.github/workflows/playground.yml) builds the
WASM and publishes `playground/web/` to GitHub Pages on every push to `main`. Enable it
once under **Settings → Pages → Source: GitHub Actions**; after that the playground is
live at your Pages URL, ready to link from the README and launch posts.

## Recording the demo GIF from it

The playground is a normal web page, so record it natively on any OS — no `ttyd`/WSL:

- **Quick:** [ScreenToGif](https://www.screentogif.com/) (Windows), Kap (macOS), or Peek
  (Linux) — capture the two panes as you type.
- **Reproducible:** a Playwright script that types into `#editor` and records video, then
  `ffmpeg` to GIF — the browser equivalent of a VHS tape, and it runs on Windows.

## What it does / doesn't do

- **Does:** parse, type/effect/exhaustiveness-check, and lower to Python — everything the
  compiler does up to emitting source, with live diagnostics (the resilient `analyze`).
- **Doesn't (yet):** *run* the emitted Python. Executing it in-browser needs Pyodide
  (CPython-in-WASM); that's a deliberate later tier. Today it shows the compiled Python,
  which is the differentiator.
