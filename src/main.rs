//! Phase 3 CLI: type-check, compile to Python, or echo the canonical form.
//!
//! Hand-rolled argument handling; the full clap subcommand surface (`fmt`,
//! `lsp`, …) arrives later — see `DESIGN.md` §10.
//!
//! Usage:
//!   pyfun check   <file.pyfun>                  # type-check, report diagnostics
//!   pyfun compile <file.pyfun> [-o <out.py>]    # type-check then lower to Python
//!   pyfun parse   <file.pyfun>                  # canonical pretty-print
//!   pyfun <file.pyfun>                          # shorthand for `compile`

use std::process::ExitCode;

use pyfun::diagnostics::{self, Level};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("-h") | Some("--help") => {
            help();
            ExitCode::SUCCESS
        }
        Some("check") => match args.get(1) {
            Some(path) => check(path),
            None => fail("`check` needs a file path"),
        },
        Some("compile") => match parse_compile_args(&args[1..]) {
            Ok((path, out)) => compile(path, out.as_deref()),
            Err(msg) => fail(&msg),
        },
        Some("parse") => match args.get(1) {
            Some(path) => parse_only(path),
            None => fail("`parse` needs a file path"),
        },
        // Shorthand: a bare path means `compile <path>` to stdout.
        Some(path) => compile(path, None),
    }
}

fn help() {
    eprintln!("pyfun {}", env!("CARGO_PKG_VERSION"));
    eprintln!();
    eprintln!("usage:");
    eprintln!("  pyfun check   <file.pyfun>                type-check, report diagnostics");
    eprintln!("  pyfun compile <file.pyfun> [-o <out.py>]  type-check then lower to Python");
    eprintln!("  pyfun parse   <file.pyfun>                canonical pretty-print");
    eprintln!("  pyfun <file.pyfun>                        shorthand for `compile`");
}

fn check(path: &str) -> ExitCode {
    let Some(source) = read(path) else {
        return ExitCode::FAILURE;
    };
    let module = match pyfun::parse(&source) {
        Ok(m) => m,
        Err(e) => {
            eprintln!(
                "{}",
                diagnostics::render(&source, Level::Error, &e.message(), e.span())
            );
            return ExitCode::FAILURE;
        }
    };
    match pyfun::types::check(&module) {
        Ok(()) => {
            eprintln!("ok: no type errors");
            ExitCode::SUCCESS
        }
        Err(errors) => {
            for (i, e) in errors.iter().enumerate() {
                if i > 0 {
                    eprintln!();
                }
                eprintln!(
                    "{}",
                    diagnostics::render(&source, Level::Error, &e.message, e.span)
                );
            }
            let n = errors.len();
            eprintln!("\n{n} error{}", if n == 1 { "" } else { "s" });
            ExitCode::FAILURE
        }
    }
}

fn compile(path: &str, out: Option<&str>) -> ExitCode {
    let Some(source) = read(path) else {
        return ExitCode::FAILURE;
    };
    let python = match pyfun::compile(&source) {
        Ok(py) => py,
        Err(e) => {
            eprintln!(
                "{}",
                diagnostics::render(&source, Level::Error, &e.message(), e.span())
            );
            return ExitCode::FAILURE;
        }
    };
    match out {
        Some(out_path) => match std::fs::write(out_path, python) {
            Ok(()) => {
                eprintln!("wrote {out_path}");
                ExitCode::SUCCESS
            }
            Err(e) => fail(&format!("cannot write {out_path}: {e}")),
        },
        None => {
            print!("{python}");
            ExitCode::SUCCESS
        }
    }
}

fn parse_only(path: &str) -> ExitCode {
    let Some(source) = read(path) else {
        return ExitCode::FAILURE;
    };
    match pyfun::format(&source) {
        Ok(canonical) => {
            print!("{canonical}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!(
                "{}",
                diagnostics::render(&source, Level::Error, &e.message(), e.span())
            );
            ExitCode::FAILURE
        }
    }
}

/// Parse `compile` arguments: a required path and an optional `-o <out>`.
fn parse_compile_args(args: &[String]) -> Result<(&str, Option<String>), String> {
    let mut path = None;
    let mut out = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => {
                i += 1;
                out = Some(args.get(i).ok_or("`-o` needs a path")?.clone());
            }
            p if path.is_none() => path = Some(p),
            other => return Err(format!("unexpected argument `{other}`")),
        }
        i += 1;
    }
    Ok((path.ok_or("`compile` needs a file path")?, out))
}

fn read(path: &str) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("error: cannot read {path}: {e}");
            None
        }
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("error: {message}");
    ExitCode::FAILURE
}
