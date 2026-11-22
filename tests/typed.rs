//! Typed output (`pyfun compile --typed`, `DESIGN.md` §5.7) and docstrings
//! (`DESIGN.md` §9). What a type checker makes of the output is checked with
//! mypy in `tests/typed_output.py`; these pin the text.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn typed(source: &str) -> String {
    pyfun::compile_typed(source)
        .unwrap_or_else(|e| panic!("compiles: {e}\n{source}"))
        .0
}

fn plain(source: &str) -> String {
    pyfun::compile(source).unwrap_or_else(|e| panic!("compiles: {e}\n{source}"))
}

/// The lines of `python` that declare something: a `def`, a `class`, a `type`
/// alias, an import or an annotated value.
fn declarations(python: &str) -> Vec<&str> {
    python
        .lines()
        .filter(|line| {
            ["def ", "class ", "type ", "from ", "import "]
                .iter()
                .any(|lead| line.starts_with(lead))
                || (!line.starts_with(' ') && line.contains(": ") && line.contains(" = "))
        })
        .filter(|line| !line.starts_with("def _pf_"))
        .collect()
}

#[test]
fn a_function_carries_its_parameters_and_its_result() {
    let python = typed("let shout times word = String.repeat times (String.upper word)");
    assert_eq!(
        declarations(&python),
        vec![
            "from __future__ import annotations",
            "def shout(times: int, word: str) -> str:",
        ]
    );
}

#[test]
fn a_type_variable_is_a_type_parameter() {
    let python = typed("let pair x y = (x, y)\nlet first xs = List.head xs");
    let decls = declarations(&python);
    assert!(
        decls.contains(&"def pair[A, B](x: A, y: B) -> tuple[A, B]:"),
        "{python}"
    );
    assert!(
        decls.contains(&"def first[A](xs: list[A]) -> Some[A] | None_:"),
        "{python}"
    );
    // The prelude's `Some` is generic to match.
    assert!(decls.contains(&"class Some[A]:"), "{python}");
    assert!(python.contains("class Some[A]:\n    _0: A\n"), "{python}");
}

#[test]
fn a_number_a_function_is_polymorphic_in_is_held_to_int_and_float() {
    // `double 2` is an `int` and `double 2.5` a `float`, which one shared
    // parameter says and `float` alone would not.
    let python = typed("let double x = x * 2\nlet count xs = List.len xs");
    let decls = declarations(&python);
    assert!(
        decls.contains(&"def double[A: (int, float)](x: A) -> A:"),
        "{python}"
    );
    assert!(
        decls.contains(&"def count[A](xs: list[A]) -> int:"),
        "{python}"
    );
}

#[test]
fn a_function_argument_is_a_callable_of_all_its_arguments() {
    let python = typed("let apply2 f x y = f x y");
    assert!(
        python.contains("def apply2[A, B, C](f: Callable[[A, B], C], x: A, y: B) -> C:"),
        "{python}"
    );
    assert!(
        python.contains("from collections.abc import Callable\n"),
        "{python}"
    );
}

#[test]
fn a_value_carries_its_type_and_a_generalized_one_is_any() {
    let python = typed(
        "type Point = { x: int, y: int }\nlet origin = Point { x = 0, y = 0 }\nlet names = [\"a\"]\nlet nothing = []",
    );
    let decls = declarations(&python);
    assert!(decls.contains(&"origin: Point = Point(0, 0)"), "{python}");
    assert!(decls.contains(&"names: list[str] = [\"a\"]"), "{python}");
    assert!(decls.contains(&"nothing: list[Any] = []"), "{python}");
    assert!(decls.contains(&"from typing import Any"), "{python}");
}

#[test]
fn classes_carry_their_fields_and_a_sum_type_gets_an_alias() {
    let python = typed(
        "type Shape =\n    | Circle(float)\n    | Rect { width: float, height: float }\n    | Dot\ntype Tree a = Leaf | Node (Tree a) a (Tree a)\ntype Pair a b = { left: a, right: b }\nlet depth tree =\n    match tree:\n        case Leaf: 0\n        case Node l _ r: 1 + max (depth l) (depth r)\nlet area shape =\n    match shape:\n        case Circle(r): 3.14 * r * r\n        case Rect { width, height }: width * height\n        case Dot: 0.0\nlet swap p = Pair { left = p.right, right = p.left }",
    );
    let decls = declarations(&python);
    for expected in [
        "class Circle:",
        "class Rect:",
        "type Shape = Circle | Rect | Dot",
        "class Node[A]:",
        "type Tree[A] = Leaf | Node[A]",
        "class Pair[A, B]:",
        "def depth[A](tree: Tree[A]) -> int:",
        "def area(shape: Shape) -> float:",
        "def swap[A, B](p: Pair[A, B]) -> Pair[B, A]:",
    ] {
        assert!(
            decls.contains(&expected),
            "missing `{expected}` in:\n{python}"
        );
    }
    // A recursive field names the alias that is defined further down.
    assert!(
        python.contains("class Node[A]:\n    _0: Tree[A]\n    _1: A\n    _2: Tree[A]\n"),
        "{python}"
    );
}

#[test]
fn collections_effects_and_units_are_spelled_as_they_run() {
    let python = typed(
        "measure m\nlet lengths = [1.5<m>, 2.0<m>]\nlet index = {\"a\": 1}\nlet seen = {1, 2}\nlet stream = seq { yield 1 }\nlet later = async { return 1 }\nlet greet name = print (String.upper name)\nlet parse text = String.to_int text |> Option.to_result \"not a number\"",
    );
    let decls = declarations(&python);
    for expected in [
        "lengths: list[float] = [1.5, 2.0]",
        "index: dict[str, int] = {\"a\": 1}",
        "seen: set[int] = {1, 2}",
        "def greet(name: str) -> None:",
        "def parse(text: str) -> Ok[int] | Error[str]:",
    ] {
        assert!(
            decls.contains(&expected),
            "missing `{expected}` in:\n{python}"
        );
    }
    assert!(python.contains("stream: Iterable[int] = "), "{python}");
    assert!(python.contains("later: Awaitable[int] = "), "{python}");
}

#[test]
fn a_newtype_is_its_underlying_type_and_an_opaque_handle_is_any() {
    let python = typed(
        "opaque type UserId = str\nextern type Conn\nextern connect : str -> Conn = sqlite3.connect\nextern import sqlite3\nlet tag (UserId raw) = String.upper raw\nlet open path = connect path",
    );
    assert!(python.contains("def tag(raw: str) -> str:"), "{python}");
    assert!(python.contains("def open(path: str) -> Any:"), "{python}");
}

#[test]
fn the_members_of_an_in_file_module_are_annotated() {
    let python = typed("module Geom =\n    let square x = x * x\n    let unit = 1.0");
    assert!(
        python.contains("def Geom_square[A: (int, float)](x: A) -> A:"),
        "{python}"
    );
    assert!(python.contains("Geom_unit: float = 1.0"), "{python}");
}

#[test]
fn typed_output_changes_nothing_but_annotations() {
    // Take the annotations away again and the plain output is left.
    let source = "type Point = { x: int, y: int }\nlet shift dx p = { p with x = p.x + dx }\nlet origin = Point { x = 0, y = 0 }\nprint (shift 2 origin)";
    let python = typed(source);
    let stripped: Vec<String> = python
        .lines()
        .filter(|line| *line != "from __future__ import annotations")
        .map(|line| {
            line.replace(
                "def shift(dx: int, p: Point) -> Point:",
                "def shift(dx, p):",
            )
            .replace("origin: Point = ", "origin = ")
        })
        .collect();
    assert_eq!(stripped.join("\n") + "\n", plain(source));
}

// ---------- Docstrings ----------

#[test]
fn a_doc_comment_is_its_functions_docstring_in_every_output() {
    let source = "## The area of a square.\nlet area side = side * side";
    assert!(
        plain(source).contains(
            "def area(side):\n    \"\"\"The area of a square.\"\"\"\n    return side * side\n"
        ),
        "{}",
        plain(source)
    );
    assert!(
        typed(source).contains("(side: A) -> A:\n    \"\"\"The area of a square.\"\"\"\n"),
        "{}",
        typed(source)
    );
}

#[test]
fn a_longer_doc_comment_closes_on_its_own_line() {
    let source = "## Join the words.\n##\n## Empty words are kept.\nlet join words = String.join \" \" words";
    assert!(
        plain(source).contains(
            "def join(words):\n    \"\"\"Join the words.\n\n    Empty words are kept.\n    \"\"\"\n    return "
        ),
        "{}",
        plain(source)
    );
}

#[test]
fn a_docstring_keeps_its_quotes_and_backslashes() {
    let source = "## Say \"hi\", or a \\n.\nlet hi name = name";
    let python = plain(source);
    assert!(
        python.contains("    \"\"\"Say \"hi\", or a \\\\n.\"\"\"\n"),
        "{python}"
    );
}

#[test]
fn a_documented_record_documents_its_class() {
    let source = "## A point on the grid.\ntype Point = { x: int, y: int }\nlet origin = Point { x = 0, y = 0 }";
    assert!(
        plain(source).contains(
            "class Point:\n    \"\"\"A point on the grid.\"\"\"\n    x: int\n    y: int\n"
        ),
        "{}",
        plain(source)
    );
}

// ---------- The command line ----------

fn pyfun(args: &[&str], cwd: &PathBuf) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_pyfun"))
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawn pyfun");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&out.stderr).replace("\r\n", "\n"),
    )
}

fn workdir(tag: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pyfun_typed_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for (name, source) in files {
        fs::write(dir.join(name), source).unwrap();
    }
    dir
}

#[test]
fn the_flag_types_a_single_file() {
    let dir = workdir("one", &[("sq.pyfun", "let square x = x * x\n")]);
    let (ok, stdout, stderr) = pyfun(&["compile", "--typed", "sq.pyfun"], &dir);
    let _ = fs::remove_dir_all(&dir);
    assert!(ok, "{stderr}");
    assert!(
        stdout.contains("def square[A: (int, float)](x: A) -> A:"),
        "{stdout}"
    );
}

#[test]
fn a_typed_project_names_other_modules_types_and_ships_the_marker() {
    let dir = workdir(
        "proj",
        &[
            (
                "geo.pyfun",
                "type Shape = Circle(float) | Square(float)\ntype Pt = { x: int, y: int }\nlet area s =\n    match s:\n        case Circle(r): 3.14 * r * r\n        case Square(w): w * w\n",
            ),
            (
                "use.pyfun",
                "import Geo\n\nlet biggest shapes = List.max_by Geo.area shapes\nlet origin = Geo.Pt { x = 0, y = 0 }\n",
            ),
        ],
    );
    let (ok, _, stderr) = pyfun(&["compile", "--typed", "use.pyfun", "-o", "out"], &dir);
    assert!(ok, "{stderr}");
    let read = |name: &str| fs::read_to_string(dir.join("out").join(name)).unwrap();
    let user = read("use.py").replace("\r\n", "\n");
    let geo = read("geo.py").replace("\r\n", "\n");
    let runtime = read("_pyfun_rt.py").replace("\r\n", "\n");
    let marker = fs::read_to_string(dir.join("out").join("py.typed"));
    let _ = fs::remove_dir_all(&dir);
    assert!(
        user.contains("def biggest(shapes: list[geo.Shape]) -> Some[geo.Shape] | None_:"),
        "{user}"
    );
    assert!(user.contains("origin: geo.Pt = geo.Pt(0, 0)"), "{user}");
    assert!(geo.contains("type Shape = Circle | Square"), "{geo}");
    assert!(geo.contains("def area(s: Shape) -> float:"), "{geo}");
    assert!(runtime.contains("class Some[A]:"), "{runtime}");
    assert_eq!(
        marker.ok().as_deref(),
        Some(""),
        "py.typed is an empty marker"
    );
}

#[test]
fn an_untyped_project_has_no_marker() {
    let dir = workdir(
        "plainproj",
        &[
            ("geo.pyfun", "let square x = x * x\n"),
            ("use.pyfun", "import Geo\n\nlet nine = Geo.square 3\n"),
        ],
    );
    let (ok, _, stderr) = pyfun(&["compile", "use.pyfun", "-o", "out"], &dir);
    let marked = dir.join("out").join("py.typed").exists();
    let _ = fs::remove_dir_all(&dir);
    assert!(ok, "{stderr}");
    assert!(!marked);
}

#[test]
fn typed_does_not_combine_with_native_or_an_older_target() {
    let dir = workdir("flags", &[("sq.pyfun", "let square x = x * x\n")]);
    let (ok, _, stderr) = pyfun(&["compile", "--typed", "--native", "sq.pyfun"], &dir);
    assert!(!ok);
    assert!(
        stderr.contains("`--typed` and `--native` are two outputs"),
        "{stderr}"
    );
    let (ok, _, stderr) = pyfun(
        &["compile", "--typed", "--target", "3.11", "sq.pyfun"],
        &dir,
    );
    let _ = fs::remove_dir_all(&dir);
    assert!(!ok);
    assert!(
        stderr.contains("cannot be combined with `--target 3.11`"),
        "{stderr}"
    );
}

#[test]
fn a_typed_program_runs_as_the_plain_one_does() {
    let have_python = ["python", "python3"]
        .iter()
        .any(|p| Command::new(p).arg("--version").output().is_ok());
    if !have_python {
        eprintln!("skipping: no python interpreter found");
        return;
    }
    let source = "type Shape =\n    | Circle(float)\n    | Rect { width: float, height: float }\ntype Tree a = Leaf | Node (Tree a) a (Tree a)\n## How deep a tree goes.\nlet depth tree =\n    match tree:\n        case Leaf: 0\n        case Node l _ r: 1 + max (depth l) (depth r)\nlet area shape =\n    match shape:\n        case Circle(r): 3.0 * r * r\n        case Rect { width, height }: width * height\nlet double x = x * 2\nprint (depth (Node Leaf 1 (Node Leaf 2 Leaf)))\nprint (area (Rect { width = 2.0, height = 3.0 }))\nprint (double 2)\nprint (double 2.5)\nprint (List.head [1, 2])\n";
    let dir = workdir("run", &[("prog.pyfun", source)]);
    let (ok, _, stderr) = pyfun(
        &["compile", "--typed", "prog.pyfun", "-o", "typed.py"],
        &dir,
    );
    assert!(ok, "{stderr}");
    let (ok, _, stderr) = pyfun(&["compile", "prog.pyfun", "-o", "plain.py"], &dir);
    assert!(ok, "{stderr}");
    let python = if Command::new("python").arg("--version").output().is_ok() {
        "python"
    } else {
        "python3"
    };
    let run = |file: &str| {
        let out = Command::new(python)
            .arg(file)
            .current_dir(&dir)
            .output()
            .expect("spawn python");
        (
            String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n"),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    };
    let (typed_out, typed_err) = run("typed.py");
    let (plain_out, _) = run("plain.py");
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(typed_err, "", "the typed program runs cleanly");
    assert_eq!(typed_out, plain_out);
    assert_eq!(typed_out, "2\n6.0\n4\n5.0\nSome(1)\n");
}
