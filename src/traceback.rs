//! Tracebacks in `.pyfun` lines (`DESIGN.md` §9): what `pyfun run` stages beside
//! the Python it emitted so that an uncaught error names the source.
//!
//! Three files join the emitted program in its temp directory: a launcher (the
//! script the interpreter is given), the hook module it imports, and a table of
//! the [`LineMap`] of every emitted file. The emitted Python itself is the
//! program `pyfun compile` writes, byte for byte; nothing is added to it.

use crate::LineMap;
use crate::lsp::json::{self, Json};

/// The script `pyfun run` hands the interpreter in place of the entry program.
pub const LAUNCHER_FILE: &str = "_pyfun_run.py";
/// The hook module the launcher imports.
pub const HOOK_FILE: &str = "_pyfun_trace.py";
/// The table of line maps the hook reads when an error reaches the top.
pub const TABLE_FILE: &str = "_pyfun_lines.json";

/// The hook module's source.
pub const HOOK_SOURCE: &str = include_str!("traceback_hook.py");

/// The launcher's source: run the staged `entry_py` as `__main__`.
pub fn launcher(entry_py: &str) -> String {
    format!("import _pyfun_trace\n\n_pyfun_trace.run({entry_py:?})\n")
}

/// One emitted file and the source it came from.
pub struct Traced<'a> {
    /// The emitted file's name in the staging directory (`_pyfun_main.py`).
    pub py_name: &'a str,
    /// The source path to show in a frame, as the user would write it.
    pub source_path: &'a str,
    /// The source text.
    pub source: &'a str,
    /// Where each emitted line came from.
    pub map: &'a LineMap,
}

/// The 1-based line of `source` that holds byte `offset`.
fn line_of(source: &str, offset: usize) -> usize {
    let upto = offset.min(source.len());
    source.as_bytes()[..upto]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
        + 1
}

/// The table the hook reads, as JSON: for each emitted file, the source path,
/// `[emitted line, first source line, last source line]` triples in line
/// order, and the source's lines.
pub fn table(files: &[Traced]) -> String {
    let entries = files
        .iter()
        .map(|file| {
            let mut lines: Vec<(usize, usize, usize)> = Vec::new();
            for (py_line, span) in file.map {
                let first = line_of(file.source, span.start);
                let last = line_of(file.source, span.end.saturating_sub(1)).max(first);
                lines.push((*py_line, first, last));
            }
            let entry = json::obj(vec![
                ("source", json::str(file.source_path)),
                (
                    "lines",
                    Json::Array(
                        lines
                            .into_iter()
                            .map(|(py, first, last)| {
                                Json::Array(vec![
                                    json::int(py as i64),
                                    json::int(first as i64),
                                    json::int(last as i64),
                                ])
                            })
                            .collect(),
                    ),
                ),
                (
                    "text",
                    Json::Array(file.source.lines().map(json::str).collect()),
                ),
            ]);
            (file.py_name.to_string(), entry)
        })
        .collect();
    Json::Object(entries).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_names_source_lines() {
        let source = "let a = 1\nlet b = 2\n\nprint (a + b)\n";
        let span = |start, end| crate::lexer::Span::new(start, end);
        let map: LineMap = vec![(1, span(0, 9)), (2, span(10, 19)), (3, span(21, 34))];
        let text = table(&[Traced {
            py_name: "_pyfun_main.py",
            source_path: "sum.pyfun",
            source,
            map: &map,
        }]);
        let parsed = json::parse(&text).expect("valid JSON");
        let entry = parsed.get("_pyfun_main.py").expect("the emitted file");
        assert_eq!(
            entry.get("source").and_then(Json::as_str),
            Some("sum.pyfun")
        );
        let lines: Vec<(i64, i64, i64)> = entry
            .get("lines")
            .and_then(Json::as_array)
            .expect("lines")
            .iter()
            .map(|pair| {
                let row = pair.as_array().expect("a row");
                (
                    row[0].as_i64().unwrap(),
                    row[1].as_i64().unwrap(),
                    row[2].as_i64().unwrap(),
                )
            })
            .collect();
        assert_eq!(lines, vec![(1, 1, 1), (2, 2, 2), (3, 4, 4)]);
        assert_eq!(
            entry
                .get("text")
                .and_then(Json::as_array)
                .map(<[Json]>::len),
            Some(4)
        );
    }

    #[test]
    fn the_launcher_quotes_the_entry_name() {
        assert_eq!(
            launcher("_pyfun_main.py"),
            "import _pyfun_trace\n\n_pyfun_trace.run(\"_pyfun_main.py\")\n"
        );
    }
}
