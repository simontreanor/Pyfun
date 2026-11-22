//! Tracebacks in `.pyfun` lines (`DESIGN.md` §9): the table that pairs emitted
//! Python with its source, and the Python hook that reads it.
//!
//! The emitted Python is the program `pyfun compile` writes, byte for byte;
//! nothing is added to it. Beside it travels a table built from its
//! [`LineMap`]: for runs of emitted lines, the source statement they came
//! from. `pyfun run` stages the table, the hook module and a launcher in the
//! program's temp directory; the REPL and the Jupyter kernel hand the hook one
//! table for each chunk of Python they execute.

use crate::LineMap;
use crate::lsp::json::{self, Json};

/// The script `pyfun run` hands the interpreter in place of the entry program.
pub const LAUNCHER_FILE: &str = "_pyfun_run.py";
/// The hook module the launcher imports.
pub const HOOK_FILE: &str = "_pyfun_trace.py";
/// The tables the hook reads when an error reaches the top.
pub const TABLE_FILE: &str = "_pyfun_lines.json";

/// The hook module's source.
pub const HOOK_SOURCE: &str = include_str!("traceback_hook.py");

/// The launcher's source: run the staged `entry_py` as `__main__`.
pub fn launcher(entry_py: &str) -> String {
    format!("import _pyfun_trace\n\n_pyfun_trace.run({entry_py:?})\n")
}

/// One row of a table: the emitted lines from `emitted` down to the next row
/// came from one source statement.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The first emitted line the row covers, 1-based.
    pub emitted: usize,
    /// The statement's first line in its source, 1-based, or 0 for emitted
    /// lines that come from no source (a prelude helper, a class).
    pub line: usize,
    /// The statement's lines, as the source has them.
    pub text: Vec<String>,
    /// The name of the source, where it is not the table's own: a REPL entry,
    /// a notebook cell.
    pub label: Option<String>,
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
pub fn line_of(source: &str, offset: usize) -> usize {
    let upto = offset.min(source.len());
    source.as_bytes()[..upto]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
        + 1
}

/// The first and last line (1-based) of the statement at `span` in `source`.
pub fn statement_lines(source: &str, span: crate::lexer::Span) -> (usize, usize) {
    let first = line_of(source, span.start);
    let last = line_of(source, span.end.saturating_sub(1)).max(first);
    (first, last)
}

/// The rows of one emitted file: a row per entry of its line map.
fn rows_of(file: &Traced) -> Vec<Row> {
    let lines: Vec<&str> = file.source.lines().collect();
    file.map
        .iter()
        .map(|(emitted, span)| {
            let (first, last) = statement_lines(file.source, *span);
            Row {
                emitted: *emitted,
                line: first,
                text: lines
                    .iter()
                    .skip(first - 1)
                    .take(last + 1 - first)
                    .map(|line| line.to_string())
                    .collect(),
                label: None,
            }
        })
        .collect()
}

fn rows_json(rows: &[Row]) -> Json {
    Json::Array(
        rows.iter()
            .map(|row| {
                let mut cells = vec![
                    json::int(row.emitted as i64),
                    json::int(row.line as i64),
                    Json::Array(row.text.iter().map(json::str).collect()),
                ];
                if let Some(label) = &row.label {
                    cells.push(json::str(label));
                }
                Json::Array(cells)
            })
            .collect(),
    )
}

/// The tables `pyfun run` stages, as JSON: for each emitted file, its source
/// path and its rows.
pub fn table(files: &[Traced]) -> String {
    let entries = files
        .iter()
        .map(|file| {
            let entry = json::obj(vec![
                ("source", json::str(file.source_path)),
                ("rows", rows_json(&rows_of(file))),
            ]);
            (file.py_name.to_string(), entry)
        })
        .collect();
    Json::Object(entries).to_string()
}

/// The table of one chunk of Python a session executes, as one line of JSON.
/// Every row names its own source, so the table has none.
pub fn chunk_table(rows: &[Row]) -> String {
    json::obj(vec![("source", json::str("")), ("rows", rows_json(rows))]).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_files_rows_carry_each_statements_lines() {
        let source = "let a = 1\nlet b =\n    2\n\nprint (a + b)\n";
        let span = |start, end| crate::lexer::Span::new(start, end);
        let map: LineMap = vec![(1, span(0, 9)), (2, span(10, 23)), (3, span(25, 38))];
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
        let rows: Vec<(i64, i64, Vec<&str>)> = entry
            .get("rows")
            .and_then(Json::as_array)
            .expect("rows")
            .iter()
            .map(|row| {
                let row = row.as_array().expect("a row");
                (
                    row[0].as_i64().unwrap(),
                    row[1].as_i64().unwrap(),
                    row[2]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|line| line.as_str().unwrap())
                        .collect(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                (1, 1, vec!["let a = 1"]),
                (2, 2, vec!["let b =", "    2"]),
                (3, 5, vec!["print (a + b)"]),
            ]
        );
    }

    #[test]
    fn a_chunks_rows_name_their_own_source() {
        let text = chunk_table(&[
            Row {
                emitted: 1,
                line: 0,
                text: vec![],
                label: None,
            },
            Row {
                emitted: 3,
                line: 2,
                text: vec!["f 0".to_string()],
                label: Some("<input 4>".to_string()),
            },
        ]);
        assert!(!text.contains('\n'), "one line: {text}");
        assert_eq!(
            text,
            r#"{"source":"","rows":[[1,0,[]],[3,2,["f 0"],"<input 4>"]]}"#
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
