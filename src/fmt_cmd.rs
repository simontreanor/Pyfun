//! `pyfun fmt` (`DESIGN.md` §9): format files in place, or say which would change.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use pyfun::diagnostics::{self, Level};
use pyfun::fmt::{self, FormatError};

use crate::fail;

/// Run `pyfun fmt [--check] [path...]`. A path is a file or a folder, which is
/// searched for `.pyfun` files; no path is the current folder; `-` reads the
/// program from standard input and writes it formatted to standard output.
pub fn run(args: &[String]) -> ExitCode {
    let check = args.iter().any(|a| a == "--check");
    let paths: Vec<&String> = args.iter().filter(|a| *a != "--check").collect();
    if let Some(flag) = paths.iter().find(|p| p.starts_with("--")) {
        return fail(&format!(
            "`fmt` has no `{flag}` option (it takes `--check`)"
        ));
    }
    if paths.iter().any(|p| *p == "-") {
        if paths.len() > 1 || check {
            return fail("`pyfun fmt -` formats standard input on its own");
        }
        return filter();
    }
    let here = ".".to_string();
    let paths = if paths.is_empty() { vec![&here] } else { paths };
    let mut files = Vec::new();
    for path in paths {
        let path = Path::new(path);
        if path.is_dir() {
            let before = files.len();
            collect(path, &mut files);
            files[before..].sort();
        } else if path.is_file() {
            files.push(path.to_path_buf());
        } else {
            return fail(&format!("cannot find {}", path.display()));
        }
    }
    let (mut changed, mut failed) = (0, 0);
    for file in &files {
        let source = match std::fs::read_to_string(file) {
            Ok(source) => source,
            Err(e) => {
                eprintln!("error: cannot read {}: {e}", file.display());
                failed += 1;
                continue;
            }
        };
        match fmt::format(&source) {
            Ok(formatted) if formatted == source => {}
            Ok(formatted) => {
                changed += 1;
                if check {
                    println!("would format {}", file.display());
                } else if let Err(e) = std::fs::write(file, formatted) {
                    eprintln!("error: cannot write {}: {e}", file.display());
                    failed += 1;
                } else {
                    println!("formatted {}", file.display());
                }
            }
            Err(error) => {
                failed += 1;
                report(&file.display().to_string(), &source, &error);
            }
        }
    }
    let unchanged = files.len() - changed - failed;
    let verb = if check {
        "would be formatted"
    } else {
        "formatted"
    };
    let mut summary = format!(
        "{changed} {} {verb}, {unchanged} unchanged",
        files_word(changed)
    );
    if failed > 0 {
        summary.push_str(&format!(", {failed} not formatted"));
    }
    eprintln!("{summary}");
    if failed > 0 || (check && changed > 0) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn files_word(n: usize) -> &'static str {
    if n == 1 { "file" } else { "files" }
}

/// Standard input to standard output, for an editor that formats a buffer.
fn filter() -> ExitCode {
    let mut source = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut source) {
        return fail(&format!("cannot read standard input: {e}"));
    }
    match fmt::format(&source) {
        Ok(formatted) => {
            print!("{formatted}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            report("<stdin>", &source, &error);
            ExitCode::FAILURE
        }
    }
}

fn report(name: &str, source: &str, error: &FormatError) {
    match error {
        FormatError::Parse(e) => {
            eprintln!("-- {name} --");
            eprintln!(
                "{}",
                diagnostics::render_in(name, source, Level::Error, &e.message(), e.span())
            );
        }
        FormatError::Unsafe => eprintln!("error: {name}: {error}"),
    }
}

/// Every `.pyfun` file under `dir`, past hidden folders and the ones tools
/// keep what they build or install in.
fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            let skipped = name.starts_with('.')
                || name.starts_with("target")
                || matches!(name.as_ref(), "node_modules" | "__pycache__" | "venv");
            if !skipped {
                collect(&path, files);
            }
        } else if path.extension().is_some_and(|x| x == "pyfun") {
            files.push(path);
        }
    }
}
