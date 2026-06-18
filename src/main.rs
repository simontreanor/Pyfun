//! Minimal Phase 1 CLI: parse a `.pyfun` file and print its canonical form.
//!
//! The real subcommand surface (`compile`, `check`, `fmt`, `lsp`, via clap)
//! arrives in Phase 3 — see `DESIGN.md` §9/§10. For now this is just enough to
//! exercise the lexer/parser/printer on a file.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        None | Some("-h") | Some("--help") => {
            eprintln!("pyfun {}", env!("CARGO_PKG_VERSION"));
            eprintln!("usage: pyfun <file.pyfun>");
            eprintln!();
            eprintln!("Parses the file and prints its canonical (fully-parenthesized) form.");
            ExitCode::SUCCESS
        }
        Some(path) => run(path),
    }
}

fn run(path: &str) -> ExitCode {
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    match pyfun::format(&source) {
        Ok(canonical) => {
            print!("{canonical}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
