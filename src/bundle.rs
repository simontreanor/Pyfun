//! `pyfun bundle <entry.pyfun> -o <dir>`: a Pyfun program as a static web page
//! (`DESIGN.md` §6, "Browser target"). The compiled Python (one file, or the whole
//! `.py` tree of a project), any `--asset` files, a loader that boots Pyodide from
//! its CDN and runs the entry, and an `index.html` around them, so the program is
//! a shareable link with no server. `--page <fragment.html>` puts the program's
//! own markup above the output panes; the program reaches it through Pyodide's
//! `js` module (`extern import js`).

use std::path::Path;
use std::process::ExitCode;

use pyfun::project;
use pyfun::python_emitter::PyTarget;

/// The Pyodide release the loader imports; the playground pins the same one, so
/// a bundled program runs on the interpreter the docs' runnable blocks run on.
const PYODIDE_URL: &str = "https://cdn.jsdelivr.net/pyodide/v314.0.3/full/";

const LOADER: &str = include_str!("bundle_loader.js");
const INDEX: &str = include_str!("bundle_index.html");

/// The parsed `bundle` arguments: the entry file, the output directory, the
/// asset files to copy in, and an optional page fragment.
pub struct Args<'a> {
    pub entry: &'a str,
    pub out: String,
    pub assets: Vec<String>,
    pub page: Option<String>,
}

pub fn parse_args(args: &[String]) -> Result<Args<'_>, String> {
    let mut entry = None;
    let mut out = None;
    let mut assets = Vec::new();
    let mut page = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => {
                i += 1;
                out = Some(args.get(i).ok_or("`-o` needs a directory")?.clone());
            }
            "--asset" => {
                i += 1;
                assets.push(args.get(i).ok_or("`--asset` needs a file path")?.clone());
            }
            "--page" => {
                i += 1;
                page = Some(args.get(i).ok_or("`--page` needs an HTML file")?.clone());
            }
            p if entry.is_none() => entry = Some(p),
            other => return Err(format!("unexpected argument `{other}`")),
        }
        i += 1;
    }
    Ok(Args {
        entry: entry.ok_or("`bundle` needs a file path")?,
        out: out.ok_or("`bundle` needs `-o <dir>` (the page's directory)")?,
        assets,
        page,
    })
}

/// Compile the entry (a single file or the project it opens) and write the page.
pub fn run(args: Args<'_>) -> ExitCode {
    let Some(source) = crate::read(args.entry) else {
        return ExitCode::FAILURE;
    };
    // The compiled files (name, source) and the entry module's file name.
    let (files, entry_py): (Vec<(String, String)>, String) = match pyfun::parse(&source) {
        Ok(module) if crate::has_imports(&module) => {
            let project = match crate::resolve_project(args.entry) {
                Ok(p) => p,
                Err(code) => return code,
            };
            if !crate::check_project_ok(&project) {
                return ExitCode::FAILURE;
            }
            let files = match crate::lower_project(&project, PyTarget::default()) {
                Ok(f) => f,
                Err(code) => return code,
            };
            let entry = project::module_py_name(&project.entry().name);
            (files, entry)
        }
        _ => match pyfun::compile_collecting(&source, PyTarget::default()) {
            Ok((py, notes)) => {
                crate::report_notes(&notes);
                (vec![("main.py".to_string(), py)], "main.py".to_string())
            }
            Err(e) => {
                eprintln!(
                    "{}",
                    pyfun::diagnostics::render(
                        &source,
                        pyfun::diagnostics::Level::Error,
                        &e.message(),
                        e.span()
                    )
                );
                return ExitCode::FAILURE;
            }
        },
    };
    let out = Path::new(&args.out);
    if let Err(e) = std::fs::create_dir_all(out) {
        return crate::fail(&format!("cannot create {}: {e}", out.display()));
    }
    for (name, python) in &files {
        if let Err(e) = std::fs::write(out.join(name), python) {
            return crate::fail(&format!("cannot write {name}: {e}"));
        }
    }
    // Assets keep their file names; the loader stages them beside the modules,
    // so a program's relative paths mean what they meant on the command line.
    let mut asset_names = Vec::with_capacity(args.assets.len());
    for asset in &args.assets {
        let path = Path::new(asset);
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return crate::fail(&format!("`--asset {asset}` has no file name"));
        };
        if let Err(e) = std::fs::copy(path, out.join(name)) {
            return crate::fail(&format!("cannot copy {asset}: {e}"));
        }
        asset_names.push(name.to_string());
    }
    let page = match &args.page {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(html) => html,
            Err(e) => return crate::fail(&format!("cannot read {path}: {e}")),
        },
        None => String::new(),
    };
    let title = Path::new(args.entry)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("pyfun")
        .to_string();
    let names: Vec<String> = files.iter().map(|(n, _)| n.clone()).collect();
    let loader = LOADER
        .replace("__PYODIDE_URL__", PYODIDE_URL)
        .replace("__FILES__", &json_strings(&names))
        .replace("__ASSETS__", &json_strings(&asset_names))
        .replace("__ENTRY__", &entry_py);
    let index = INDEX
        .replace("__TITLE__", &html_escape(&title))
        .replace("__PAGE__", &page);
    for (name, text) in [("pyfun-bundle.js", loader), ("index.html", index)] {
        if let Err(e) = std::fs::write(out.join(name), text) {
            return crate::fail(&format!("cannot write {name}: {e}"));
        }
    }
    eprintln!(
        "wrote index.html to {} ({} module{}, {} asset{}); serve the directory over http and open it",
        out.display(),
        files.len(),
        if files.len() == 1 { "" } else { "s" },
        asset_names.len(),
        if asset_names.len() == 1 { "" } else { "s" },
    );
    ExitCode::SUCCESS
}

/// A JSON array of strings (file names: escaped for the rare quote or backslash).
fn json_strings(items: &[String]) -> String {
    let quoted: Vec<String> = items
        .iter()
        .map(|s| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect();
    format!("[{}]", quoted.join(", "))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
