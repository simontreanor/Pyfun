//! Tracebacks in `.pyfun` lines (`DESIGN.md` §9): the line map from emitted
//! Python back to the source, and what `pyfun run` prints when a program fails.

use std::fs;
use std::path::Path;

/// Every ```` ```pyfun ```` block of every lesson, with the lesson's file name.
fn lesson_blocks() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/src/learn");
    let mut out = Vec::new();
    let mut names: Vec<_> = fs::read_dir(&dir)
        .expect("lessons directory")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    names.sort();
    for path in names {
        let text = fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let mut rest = text.as_str();
        while let Some(start) = rest.find("```pyfun\n") {
            let body = &rest[start + "```pyfun\n".len()..];
            let Some(end) = body.find("```") else { break };
            out.push((name.clone(), body[..end].to_string()));
            rest = &body[end + 3..];
        }
    }
    out
}

/// Every single-file example (one with no `import` of another file module).
fn single_file_examples() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let mut out = Vec::new();
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d).unwrap().filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|x| x == "pyfun") {
                let text = fs::read_to_string(&path).unwrap();
                if !text.lines().any(|l| l.starts_with("import ")) {
                    out.push((path.to_string_lossy().to_string(), text));
                }
            }
        }
    }
    out.sort();
    out
}

#[test]
fn every_program_that_compiles_gets_a_line_map() {
    // A traced lowering must emit the program the plain one does, markers
    // aside; where it does not, there is no map. This is the whole corpus the
    // repository has: if a rewrite starts reacting to a marker, it shows here.
    let mut compiled = 0;
    let mut unmapped = Vec::new();
    for (name, source) in lesson_blocks().into_iter().chain(single_file_examples()) {
        let Ok((_, map)) = pyfun::compile_with_line_map(&source) else {
            continue; // a block that is meant not to compile
        };
        compiled += 1;
        if map.is_none() {
            unmapped.push(name);
        }
    }
    assert!(compiled > 80, "only {compiled} programs compiled");
    assert!(
        unmapped.is_empty(),
        "{} of {compiled} programs have no line map: {unmapped:?}",
        unmapped.len()
    );
}

#[test]
#[ignore = "a debugging aid: prints where a traced emission departs from the real one"]
fn show_where_a_traced_emission_differs() {
    for (name, source) in single_file_examples() {
        let (Ok(plain), Ok(traced)) = (pyfun::compile(&source), pyfun::traced_python(&source))
        else {
            continue;
        };
        if plain == traced {
            continue;
        }
        let (a, b): (Vec<&str>, Vec<&str>) = (plain.lines().collect(), traced.lines().collect());
        let at = a
            .iter()
            .zip(&b)
            .position(|(x, y)| x != y)
            .unwrap_or(a.len().min(b.len()));
        println!("== {name}: first difference at line {}", at + 1);
        for i in at.saturating_sub(3)..(at + 6) {
            println!("  plain  {:>4} | {}", i + 1, a.get(i).unwrap_or(&"<end>"));
        }
        for i in at.saturating_sub(3)..(at + 6) {
            println!("  traced {:>4} | {}", i + 1, b.get(i).unwrap_or(&"<end>"));
        }
    }
}

// ---------- What `pyfun run` prints when a program fails ----------

use std::path::PathBuf;
use std::process::Command;

/// Whether a Python interpreter is available to execute emitted code.
fn have_python() -> bool {
    ["python", "python3"]
        .iter()
        .any(|p| Command::new(p).arg("--version").output().is_ok())
}

/// A fresh directory holding `files`, for one test.
fn project_dir(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pyfun_tb_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create the test directory");
    for (file, source) in files {
        fs::write(dir.join(file), source).expect("write a source file");
    }
    dir
}

/// Run `pyfun run <entry>` from inside `dir` and return what it wrote to
/// stderr, having checked that it failed. `None` when there is no Python.
fn failing_run(dir: &Path, entry: &str, env: &[(&str, &str)]) -> Option<String> {
    if !have_python() {
        eprintln!("skipping a traceback test: no python interpreter found");
        return None;
    }
    let output = Command::new(env!("CARGO_BIN_EXE_pyfun"))
        .arg("run")
        .arg(entry)
        .current_dir(dir)
        // A terminal that asks for colour must not put escapes in the text compared.
        .env("PYTHON_COLORS", "0")
        .env_remove("PYFUN_TRACEBACK")
        .envs(env.iter().copied())
        .output()
        .expect("spawn pyfun run");
    let _ = fs::remove_dir_all(dir);
    assert!(
        !output.status.success(),
        "an uncaught error must make `run` exit non-zero"
    );
    Some(String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"))
}

const FAILING: &str = "\
type Shape =
    | Circle(float)
    | Rect { width: float, height: float }

let ratio a b =
    let total = a + b
    a // (total - total)

let describe shape =
    match shape:
        case Circle(r): ratio 1 2
        case Rect { width, height }: 0

let scores =
    [1, 2, 3]
    |> List.map (fun x -> x + describe (Circle 1.0))
    |> List.filter (fun x -> x > 0)

print scores
";

#[test]
fn a_failing_program_is_reported_in_pyfun_lines() {
    let dir = project_dir("lines", &[("shapes.pyfun", FAILING)]);
    let Some(stderr) = failing_run(&dir, "shapes.pyfun", &[]) else {
        return;
    };
    // Each frame names the source file, the line and the source text: the
    // innermost statement, and the `match` arm that called it.
    assert_eq!(
        stderr,
        "\
Traceback (most recent call last):
  File \"shapes.pyfun\", line 14, in <module>
    let scores =
        [1, 2, 3]
        |> List.map (fun x -> x + describe (Circle 1.0))
        |> List.filter (fun x -> x > 0)
  File \"shapes.pyfun\", line 14, in <lambda>
    let scores =
  File \"shapes.pyfun\", line 11, in describe
    case Circle(r): ratio 1 2
  File \"shapes.pyfun\", line 7, in ratio
    a // (total - total)
ZeroDivisionError: integer division or modulo by zero
"
        .replace(
            "integer division or modulo by zero",
            // The message is Python's, and its wording varies by version.
            stderr
                .lines()
                .last()
                .and_then(|l| l.strip_prefix("ZeroDivisionError: "))
                .unwrap_or_default()
        ),
        "{stderr}"
    );
}

#[test]
fn a_frame_the_compiler_wrote_is_left_out() {
    let dir = project_dir("helpers", &[("shapes.pyfun", FAILING)]);
    let Some(stderr) = failing_run(&dir, "shapes.pyfun", &[]) else {
        return;
    };
    // `List.map` is a prelude helper in the emitted Python, and the program
    // runs under a launcher: neither is the author's code.
    for hidden in ["_pf_map", "_pyfun_main.py", "_pyfun_trace", "_pyfun_run"] {
        assert!(!stderr.contains(hidden), "`{hidden}` shows in:\n{stderr}");
    }
}

#[test]
fn pythons_own_traceback_is_one_variable_away() {
    let dir = project_dir("python", &[("shapes.pyfun", FAILING)]);
    let Some(stderr) = failing_run(&dir, "shapes.pyfun", &[("PYFUN_TRACEBACK", "python")]) else {
        return;
    };
    assert!(stderr.contains("_pyfun_main.py"), "{stderr}");
    assert!(stderr.contains("in _pf_map"), "{stderr}");
    assert!(!stderr.contains("shapes.pyfun"), "{stderr}");
}

#[test]
fn a_project_names_each_modules_file_and_keeps_library_frames() {
    let dir = project_dir(
        "project",
        &[
            (
                "reader.pyfun",
                "\
extern import json
extern loads : str -> int = json.loads

let parse text =
    let n = loads text
    n + 1
",
            ),
            (
                "app.pyfun",
                "\
import Reader

let main _ =
    let good = Reader.parse \"41\"
    print good
    Reader.parse \"not json\"

main () |> print
",
            ),
        ],
    );
    let Some(stderr) = failing_run(&dir, "app.pyfun", &[]) else {
        return;
    };
    let expected_head = "\
Traceback (most recent call last):
  File \"app.pyfun\", line 8, in <module>
    main () |> print
  File \"app.pyfun\", line 6, in main
    Reader.parse \"not json\"
  File \"reader.pyfun\", line 5, in parse
    let n = loads text
";
    assert!(stderr.starts_with(expected_head), "{stderr}");
    // The library's own frames follow, as Python shows them.
    assert!(stderr.contains("decoder.py"), "{stderr}");
    assert!(stderr.contains("JSONDecodeError"), "{stderr}");
    assert!(!stderr.contains("reader.py\""), "{stderr}");
}

#[test]
fn a_computation_expression_item_has_its_own_line() {
    let dir = project_dir(
        "ce",
        &[(
            "steps.pyfun",
            "\
let halve n =
    if n % 2 == 0 then Ok (n // 2) else Error \"odd\"

let steps start =
    result {
        let! a = halve start
        let b = 10 // (a - a)
        let! c = halve b
        return c
    }

steps 8 |> print
",
        )],
    );
    let Some(stderr) = failing_run(&dir, "steps.pyfun", &[]) else {
        return;
    };
    assert!(
        stderr.contains("  File \"steps.pyfun\", line 7, in <fun>\n    let b = 10 // (a - a)\n")
            || stderr
                .contains("  File \"steps.pyfun\", line 7, in steps\n    let b = 10 // (a - a)\n"),
        "{stderr}"
    );
    assert!(
        stderr.contains("  File \"steps.pyfun\", line 12, in <module>\n    steps 8 |> print\n"),
        "{stderr}"
    );
}

#[test]
fn a_program_that_ends_cleanly_prints_nothing_extra() {
    if !have_python() {
        return;
    }
    let dir = project_dir("clean", &[("ok.pyfun", "print \"done\"\n")]);
    let output = Command::new(env!("CARGO_BIN_EXE_pyfun"))
        .arg("run")
        .arg("ok.pyfun")
        .current_dir(&dir)
        .output()
        .expect("spawn pyfun run");
    let _ = fs::remove_dir_all(&dir);
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "done");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
}

#[test]
fn a_branch_in_value_position_has_its_own_line() {
    let dir = project_dir(
        "arms",
        &[(
            "arms.pyfun",
            "\
let risky n = 10 // n

let label found =
    let text = match found:
        case 0: \"none\"
        case _: f\"{risky (found - found)}\"
    text

label 5 |> print
",
        )],
    );
    let Some(stderr) = failing_run(&dir, "arms.pyfun", &[]) else {
        return;
    };
    assert!(
        stderr.contains(
            "  File \"arms.pyfun\", line 6, in label\n    case _: f\"{risky (found - found)}\"\n"
        ),
        "{stderr}"
    );
}

#[test]
fn a_long_statement_is_cut_short() {
    let dir = project_dir(
        "long",
        &[(
            "long.pyfun",
            "\
let risky n = 10 // n

let total =
    [1, 2, 3]
    |> List.map (fun x -> x + 1)
    |> List.map (fun x -> x + 2)
    |> List.map (fun x -> x + 3)
    |> List.map (fun x -> x + 4)
    |> List.map (fun x -> x + 5)
    |> List.map (fun x -> risky (x - x))
    |> List.sum

print total
",
        )],
    );
    let Some(stderr) = failing_run(&dir, "long.pyfun", &[]) else {
        return;
    };
    assert!(
        stderr.contains(
            "\
  File \"long.pyfun\", line 3, in <module>
    let total =
        [1, 2, 3]
        |> List.map (fun x -> x + 1)
        |> List.map (fun x -> x + 2)
        |> List.map (fun x -> x + 3)
        ...
"
        ),
        "{stderr}"
    );
}

#[test]
fn every_module_of_a_project_gets_a_line_map() {
    let entry = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/modules/main.pyfun");
    let project = pyfun::project::build_from_path(&entry).expect("the example project resolves");
    let compiled = pyfun::project::compile_traced(&project).expect("it lowers");
    let mapped: Vec<&str> = compiled
        .line_maps
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    let modules: Vec<&str> = project.modules.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(mapped, modules);
    // Tracing changes nothing about what is emitted.
    let plain = pyfun::project::compile(&project).expect("it lowers");
    assert_eq!(compiled.files, plain.files);
}
