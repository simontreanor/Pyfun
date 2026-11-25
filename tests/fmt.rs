//! `pyfun fmt` (`DESIGN.md` §9): the formatter over everything the repository
//! has, and the command itself.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every `.pyfun` file under `dir`.
fn pyfun_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d).unwrap().filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|x| x == "pyfun") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Every ```` ```pyfun ```` block of every Markdown file under `dir`.
fn blocks(dir: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d).unwrap().filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|x| x == "md") {
                let text = fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
                let mut rest = text.as_str();
                let mut n = 0;
                while let Some(start) = rest.find("```pyfun\n") {
                    let body = &rest[start + "```pyfun\n".len()..];
                    let Some(end) = body.find("```") else { break };
                    out.push((
                        format!("{} block {n}", path.display()),
                        body[..end].to_string(),
                    ));
                    rest = &body[end + 3..];
                    n += 1;
                }
            }
        }
    }
    out.sort();
    out
}

/// Every program the repository has that parses: the examples, the native
/// regression programs, and the code of the lessons and the other docs.
fn corpus() -> Vec<(String, String)> {
    let mut programs: Vec<(String, String)> = Vec::new();
    for dir in ["examples", "tests"] {
        for path in pyfun_files(&repo().join(dir)) {
            let text = fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
            programs.push((path.display().to_string(), text));
        }
    }
    programs.extend(blocks(&repo().join("docs/src")));
    programs.retain(|(_, source)| pyfun::parse(source).is_ok());
    programs
}

#[test]
fn every_program_formats_to_the_same_program_and_stays_put() {
    // The formatter checks for itself that the result parses to what the
    // source parses to, and reports the file when it cannot manage that; so
    // an `Ok` here is a program that still means what it meant.
    let programs = corpus();
    assert!(programs.len() > 150, "only {} programs", programs.len());
    for (name, source) in &programs {
        let once = pyfun::fmt::format(source).unwrap_or_else(|e| panic!("{name}: {e}"));
        let twice = pyfun::fmt::format(&once).unwrap_or_else(|e| panic!("{name}, again: {e}"));
        assert_eq!(once, twice, "{name}: formatting it again changed it");
        // Every comment is still there, in order.
        let comments = |text: &str| -> Vec<String> {
            text.lines()
                .filter_map(|l| l.trim().strip_prefix('#').map(str::to_string))
                .collect()
        };
        assert_eq!(comments(source), comments(&once), "{name}: comment lines");
        assert_eq!(
            source.lines().filter(|l| !l.trim().is_empty()).count(),
            once.lines().filter(|l| !l.trim().is_empty()).count(),
            "{name}: a line was joined or split"
        );
    }
}

#[test]
fn a_program_with_its_indentation_shaken_formats_back() {
    // Every program that is already formatted, with each line's indentation
    // tripled, and again with it stretched unevenly: the blocks are the same
    // blocks, so the formatter must give back the text it started from.
    let mut shaken_programs = 0;
    for (name, source) in corpus() {
        // Inside a multi-line string a line's indentation is the string's own.
        if source.contains("\"\"\"") || pyfun::fmt::format(&source).ok().as_deref() != Some(&source)
        {
            continue;
        }
        let stretches: [fn(usize) -> usize; 2] = [|n| n * 3, |n| n / 2 * 5];
        for stretch in stretches {
            let shaken: String = source
                .lines()
                .map(|line| {
                    let body = line.trim_start_matches(' ');
                    let indent = line.len() - body.len();
                    format!("{}{body}   \n", " ".repeat(stretch(indent)))
                })
                .collect();
            let formatted = pyfun::fmt::format(&shaken).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(formatted, source, "{name}");
        }
        shaken_programs += 1;
    }
    assert!(shaken_programs > 100, "only {shaken_programs} programs");
}

#[test]
fn the_lessons_and_examples_are_formatted() {
    // What the repository teaches from is written the way the formatter
    // writes it.
    let mut unformatted = Vec::new();
    let mut programs: Vec<(String, String)> = pyfun_files(&repo().join("examples"))
        .into_iter()
        .map(|p| {
            let text = fs::read_to_string(&p).unwrap().replace("\r\n", "\n");
            (p.display().to_string(), text)
        })
        .collect();
    programs.extend(blocks(&repo().join("docs/src/learn")));
    for (name, source) in programs {
        if let Ok(formatted) = pyfun::fmt::format(&source)
            && formatted != source
        {
            unformatted.push(name);
        }
    }
    assert!(unformatted.is_empty(), "run `pyfun fmt`: {unformatted:#?}");
}

// ---- a layout the author chose ----

/// `source` formatted, which formatting again leaves alone.
fn formatted(source: &str) -> String {
    let once = pyfun::fmt::format(source).unwrap_or_else(|e| panic!("{e}\n{source}"));
    let twice = pyfun::fmt::format(&once).unwrap_or_else(|e| panic!("{e}\n{once}"));
    assert_eq!(once, twice, "formatting it again changed it");
    once
}

#[test]
fn a_leading_comma_list_keeps_the_space_that_lines_it_up() {
    // `[ a` over `, b` sets the elements in one column, and the space after
    // the bracket is what does it. The spaces between the elements of a line
    // are settled as everywhere else.
    let source = "\
let quadrant =
  [ (0, 0, \"QW\")
  , (0, 7, \"TW\"),   (3, 3, \"TW\"),   (3, 10, \"TW\")
  , (1, 1, \"DW\"),   (1, 8, \"DW\"),   (2, 2, \"DW\") ]
";
    assert_eq!(
        formatted(source),
        "\
let quadrant =
  [ (0, 0, \"QW\")
  , (0, 7, \"TW\"), (3, 3, \"TW\"), (3, 10, \"TW\")
  , (1, 1, \"DW\"), (1, 8, \"DW\"), (2, 2, \"DW\") ]
"
    );
    // The same for a tuple and a map, and for a closing bracket on its own line.
    let source = "\
let pair =
  ( 1
  , 2 )
let table =
  { \"a\": 1
  , \"b\": 2 }
let names =
  [ \"ada\"
  , \"grace\"
  ]
";
    assert_eq!(formatted(source), source);
    // The space is the author's to leave out, too.
    let source = "let tight =\n  [(0, 0)\n  , (1, 1)]\n";
    assert_eq!(formatted(source), source);
    // A list with its commas at the ends of the lines, or on one line, has no
    // column to keep.
    assert_eq!(
        formatted("let xs = [ 1,\n  2 ]\nlet ys = [ 1, 2 ]\n"),
        "let xs = [1,\n  2]\nlet ys = [1, 2]\n"
    );
    // Only the bracket with the leading comma keeps its spaces: one inside it
    // is spaced by rule.
    assert_eq!(
        formatted("let rows =\n  [ [ 1, 2 ]\n  , [ 3, 4 ] ]\n"),
        "let rows =\n  [ [1, 2]\n  , [3, 4] ]\n"
    );
}

#[test]
fn a_record_types_braces_are_spaced_on_whichever_line_they_open() {
    let spaced = "\
type Hello =
  { version: int, name: str
  , moves: List str }

type Point = { x: int, y: int }
";
    assert_eq!(formatted(spaced), spaced);
    assert_eq!(
        formatted(
            "type Hello =\n  {version: int, name: str\n  , moves: List str}\n\ntype Point = {x: int, y: int}\n"
        ),
        spaced
    );
    // A field per line.
    assert_eq!(
        formatted("type Hello =\n    {\n        version: int,\n        name: str\n    }\n"),
        "type Hello =\n  {\n    version: int,\n    name: str\n  }\n"
    );
    assert_eq!(
        formatted("type Hello =\n  {version: int,\n   name: str}\n"),
        "type Hello =\n  { version: int,\n    name: str }\n"
    );
    // The declaration ends where the next statement begins: a map there is
    // still a map.
    assert_eq!(
        formatted("type Hello =\n  {name: str}\nlet ages = { \"ada\": 36 }\n"),
        "type Hello =\n  { name: str }\nlet ages = {\"ada\": 36}\n"
    );
}

#[test]
fn lines_hung_at_three_depths_keep_them() {
    // The `else` stands under its `if` and the branch deeper still: three
    // columns, where one step of indentation has room for two.
    let hung = "\
let swap letters bag =
  result {
    do! if List.len letters == 0:
          Error \"Say which tiles.\"
        else Ok ()
    return List.concat bag letters
  }
";
    assert_eq!(formatted(hung), hung);
    // The block around them is indented again, and they move with the line
    // they hang from.
    let wide = "\
let swap letters bag =
    result {
        do! if List.len letters == 0:
              Error \"Say which tiles.\"
            else Ok ()
        return List.concat bag letters
    }
";
    assert_eq!(formatted(wide), hung);
    // The same inside parentheses, under the line that opens the `if`.
    let hung = "\
let pick c =
  let y = (
    if c:
        1
      else:
        2)
  y
";
    assert_eq!(formatted(hung), hung);
    // Only under a line that itself stands inside brackets. Anywhere else an
    // odd column is set on a level, as it always was: the uneven steps of a
    // pipeline, an argument under the first one, and the items of a builder
    // under the line that opens it.
    assert_eq!(
        formatted(
            "let f xs =\n  xs\n      |> List.map inc\n    |> List.filter big\n  |> List.sum\n"
        ),
        "let f xs =\n  xs\n    |> List.map inc\n    |> List.filter big\n  |> List.sum\n"
    );
    assert_eq!(
        formatted("let label =\n  join (prefix\n          |> upper)\n       suffix\n"),
        "let label =\n  join (prefix\n    |> upper)\n    suffix\n"
    );
    assert_eq!(
        formatted("let g =\n  async {\n      let a = 1\n    let b = 2\n      return a + b\n  }\n"),
        "let g =\n  async {\n    let a = 1\n    let b = 2\n      return a + b\n  }\n"
    );
    // Lines that step back onto a level are the ordinary staircase: a step
    // each, however far they hung.
    assert_eq!(
        formatted("let total =\n    add (one\n            |> double)\n    |> print\n"),
        "let total =\n  add (one\n    |> double)\n  |> print\n"
    );
}

#[test]
fn a_leading_comma_list_keeps_its_columns_wherever_its_bracket_stands() {
    // An item's argument one step in from the item, a comma under the brace
    // of `Cell {`, and a comma set so that the fields after `with` line up.
    let source = "\
type Entry = { keys: List str, blurb: str }
type Cmd = Help Entry | Quit Entry
type Cell = { row: int, col: int, letter: str }

let table =
  [ Help
      { keys = [\"HELP\", \"\"]
      , blurb = \"this message\" }
  , Quit
      { keys = [\"QUIT\"]
      , blurb = \"end the game now\" } ]

let cell r c ch =
  let up = String.upper ch
  Cell { row = r, col = c
       , letter = up }

let moved cell =
  { cell with row = cell.row + 1
            , col = cell.col + 1 }
";
    assert_eq!(formatted(source), source);
    // Indented again, the lines move with what they hang from.
    let wide = "\
type Entry = { keys: List str, blurb: str }
type Cmd = Help Entry | Quit Entry
type Cell = { row: int, col: int, letter: str }

let table =
    [ Help
            { keys = [\"HELP\", \"\"]
            , blurb = \"this message\" }
    , Quit
            { keys = [\"QUIT\"]
            , blurb = \"end the game now\" } ]

let cell r c ch =
    let up = String.upper ch
    Cell { row = r, col = c
         , letter = up }

let moved cell =
    { cell with row = cell.row + 1
              , col = cell.col + 1 }
";
    assert_eq!(formatted(wide), source);
    // A comma under its brace, where the brace is two columns in.
    let source = "let play g =\n  ({ g with x = 1\n   , y = 2 }, 3)\ntype G = { x: int, y: int }\n";
    assert_eq!(formatted(source), source);
    // A comma the author set under neither is set on a level, as before.
    assert_eq!(
        formatted(
            "let cell r =\n  Cell { row = r\n      , col = r }\ntype Cell = { row: int, col: int }\n"
        ),
        "let cell r =\n  Cell { row = r\n    , col = r }\ntype Cell = { row: int, col: int }\n"
    );
    // A line deeper than an item goes on with the item only inside its list:
    // after the list closes, a pipeline step takes its level.
    assert_eq!(
        formatted("let xs =\n  [ 1\n      + 2\n  , 3 ]\n      |> List.sum\n"),
        "let xs =\n  [ 1\n      + 2\n  , 3 ]\n    |> List.sum\n"
    );
}

// ---- the command ----

fn pyfun_bin() -> &'static str {
    env!("CARGO_BIN_EXE_pyfun")
}

/// A scratch folder with `files`, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str, files: &[(&str, &str)]) -> Self {
        let dir = std::env::temp_dir().join(format!("pyfun_fmt_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for (name, source) in files {
            let path = dir.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, source).unwrap();
        }
        Scratch(dir)
    }

    fn read(&self, file: &str) -> String {
        fs::read_to_string(self.0.join(file)).unwrap()
    }

    fn pyfun(&self, args: &[&str]) -> (bool, String, String) {
        let out = Command::new(pyfun_bin())
            .args(args)
            .current_dir(&self.0)
            .output()
            .expect("pyfun runs");
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).replace('\\', "/"),
            String::from_utf8_lossy(&out.stderr).replace('\\', "/"),
        )
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const MESSY: &str = "let  x=1\nlet f y =\n    y+x\n";
const TIDY: &str = "let x = 1\nlet f y =\n  y + x\n";

#[test]
fn fmt_rewrites_the_files_that_need_it() {
    let scratch = Scratch::new(
        "write",
        &[
            ("a.pyfun", MESSY),
            ("b.pyfun", TIDY),
            ("shapes/c.pyfun", MESSY),
            (".hidden/d.pyfun", MESSY),
        ],
    );
    let (ok, out, err) = scratch.pyfun(&["fmt"]);
    assert!(ok, "{out}{err}");
    assert_eq!(scratch.read("a.pyfun"), TIDY);
    assert_eq!(scratch.read("shapes/c.pyfun"), TIDY);
    // A hidden folder is somebody else's.
    assert_eq!(scratch.read(".hidden/d.pyfun"), MESSY);
    assert!(out.contains("formatted ./a.pyfun"), "{out}");
    assert!(out.contains("formatted ./shapes/c.pyfun"), "{out}");
    assert!(!out.contains("b.pyfun"), "{out}");
    assert!(err.contains("2 files formatted, 1 unchanged"), "{err}");
    // A second run finds nothing to do.
    let (ok, out, err) = scratch.pyfun(&["fmt"]);
    assert!(ok);
    assert_eq!(out, "");
    assert!(err.contains("0 files formatted, 3 unchanged"), "{err}");
}

#[test]
fn check_says_what_would_change_and_changes_nothing() {
    let scratch = Scratch::new("check", &[("a.pyfun", MESSY), ("b.pyfun", TIDY)]);
    let (ok, out, err) = scratch.pyfun(&["fmt", "--check", "a.pyfun", "b.pyfun"]);
    assert!(!ok);
    assert_eq!(out, "would format a.pyfun\n");
    assert!(
        err.contains("1 file would be formatted, 1 unchanged"),
        "{err}"
    );
    assert_eq!(scratch.read("a.pyfun"), MESSY);
    let (ok, _, _) = scratch.pyfun(&["fmt", "--check", "b.pyfun"]);
    assert!(ok);
}

#[test]
fn a_file_that_does_not_parse_is_reported_and_left() {
    let broken = "let = =\n";
    let scratch = Scratch::new("broken", &[("a.pyfun", broken), ("b.pyfun", MESSY)]);
    let (ok, _, err) = scratch.pyfun(&["fmt"]);
    assert!(!ok);
    assert!(err.contains("-- ./a.pyfun --"), "{err}");
    assert!(err.contains("error:"), "{err}");
    assert!(
        err.contains("1 file formatted, 0 unchanged, 1 not formatted"),
        "{err}"
    );
    assert_eq!(scratch.read("a.pyfun"), broken);
    // The other file is still formatted.
    assert_eq!(scratch.read("b.pyfun"), TIDY);
}

#[test]
fn a_dash_formats_standard_input() {
    let mut child = Command::new(pyfun_bin())
        .args(["fmt", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("pyfun runs");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(MESSY.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), TIDY);
}

#[test]
fn a_formatted_program_runs_as_it_did() {
    // The whole of `examples/hello.pyfun`, with its layout shaken: every
    // step of indentation doubled and the spaces around `=` taken out where
    // a line has no string. It formats back, and compiles to the same Python.
    let source = fs::read_to_string(repo().join("examples/hello.pyfun"))
        .unwrap()
        .replace("\r\n", "\n");
    let shaken: String = source
        .lines()
        .map(|line| {
            let indent = line.len() - line.trim_start().len();
            let body = line.trim_start();
            let body = if body.contains('"') || body.contains('#') {
                body.to_string()
            } else {
                body.replace(" = ", "=").replace(" + ", "+")
            };
            format!("{}{body}\n", " ".repeat(indent * 2))
        })
        .collect();
    assert_ne!(shaken, source);
    let formatted = pyfun::fmt::format(&shaken).expect("formats");
    assert_eq!(formatted, source);
    assert_eq!(
        pyfun::compile(&formatted).expect("compiles"),
        pyfun::compile(&source).expect("compiles")
    );
}
