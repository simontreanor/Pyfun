//! Tests for `pyfun kernel-engine` — the framed stdio protocol behind the
//! Jupyter kernel (`src/kernel.rs`). These drive the built binary over its
//! stdin/stdout, playing the role of the `pyfun_kernel` Python package: no
//! Python interpreter is involved, because the engine only *plans* execution
//! (returning the blob the kernel would exec) — which is exactly what makes
//! the semantics testable byte-for-byte.

use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Engine {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
}

struct Reply {
    status: String,
    message: String,
    blob: String,
}

impl Engine {
    fn spawn() -> Engine {
        let mut child = Command::new(env!("CARGO_BIN_EXE_pyfun"))
            .arg("kernel-engine")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn kernel-engine");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        Engine {
            child,
            stdin,
            stdout,
        }
    }

    fn request(&mut self, op: &str, payload: &str) -> Reply {
        for part in [op, payload] {
            write!(self.stdin, "{:08}", part.len()).unwrap();
            self.stdin.write_all(part.as_bytes()).unwrap();
        }
        self.stdin.flush().unwrap();
        let mut frames = Vec::new();
        for _ in 0..3 {
            let mut header = [0u8; 8];
            self.stdout.read_exact(&mut header).unwrap();
            let len: usize = std::str::from_utf8(&header).unwrap().parse().unwrap();
            let mut body = vec![0u8; len];
            self.stdout.read_exact(&mut body).unwrap();
            frames.push(String::from_utf8(body).unwrap());
        }
        let blob = frames.pop().unwrap();
        let message = frames.pop().unwrap();
        let status = frames.pop().unwrap();
        Reply {
            status,
            message,
            blob,
        }
    }

    fn eval(&mut self, cell: &str) -> Reply {
        self.request("eval", cell)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn a_definition_echoes_its_type_and_compiles_once() {
    let mut engine = Engine::spawn();
    let reply = engine.eval("let n = 6 * 7");
    assert_eq!(reply.status, "ok");
    assert_eq!(reply.message, "n : int\n");
    assert!(reply.blob.contains("n = 6 * 7"), "{}", reply.blob);

    // Re-entering the identical definition adds nothing new to execute.
    let again = engine.eval("let n = 6 * 7");
    assert_eq!(again.status, "ok");
    assert_eq!(again.message, "n : int\n");
    assert_eq!(again.blob, "", "identical chunks must be diffed away");
}

#[test]
fn a_changed_definition_leaves_what_already_ran_alone() {
    let mut engine = Engine::spawn();
    assert_eq!(engine.eval("let n = 1\nlet f u = n").status, "ok");
    // `n` entered again as something else: only the new binding is sent. The
    // first `n` and the `f` that reads it are in the worker as they were.
    let reply = engine.eval("let n = \"two\"");
    assert_eq!(reply.status, "ok");
    assert_eq!(reply.message, "n : str\n");
    assert!(reply.blob.contains("= \"two\""), "{}", reply.blob);
    assert!(!reply.blob.contains("= 1"), "{}", reply.blob);
    assert!(!reply.blob.contains("def f"), "{}", reply.blob);
}

#[test]
fn an_expression_is_wrapped_in_print_and_reruns() {
    let mut engine = Engine::spawn();
    assert_eq!(engine.eval("let n = 41").status, "ok");
    let reply = engine.eval("n + 1");
    assert_eq!(reply.status, "ok");
    assert!(reply.blob.contains("print"), "{}", reply.blob);
    assert!(!reply.blob.contains("n = 41"), "defs already executed");

    // Unlike a definition, re-entering the expression re-runs it.
    let again = engine.eval("n + 1");
    assert!(again.blob.contains("print"), "{}", again.blob);
}

#[test]
fn a_unit_expression_runs_bare() {
    let mut engine = Engine::spawn();
    let reply = engine.eval("print \"hi\"");
    assert_eq!(reply.status, "ok");
    // The emitted statement is the print itself, not `print (print "hi")`.
    assert_eq!(reply.blob.matches("print").count(), 1, "{}", reply.blob);
}

#[test]
fn a_mixed_cell_splits_definitions_from_the_trailing_expression() {
    let mut engine = Engine::spawn();
    let reply = engine.eval("let double x = x * 2\ndouble 21");
    assert_eq!(reply.status, "ok");
    assert!(reply.message.starts_with("double : "), "{}", reply.message);
    assert!(reply.blob.contains("def double"), "{}", reply.blob);
    assert!(reply.blob.contains("print"), "{}", reply.blob);

    // Re-running the cell: the definition is diffed away, the expression stays.
    let again = engine.eval("let double x = x * 2\ndouble 21");
    assert!(!again.blob.contains("def double"), "{}", again.blob);
    assert!(again.blob.contains("print"), "{}", again.blob);
}

#[test]
fn a_type_error_reports_and_changes_nothing() {
    let mut engine = Engine::spawn();
    assert_eq!(engine.eval("let n = 1").status, "ok");
    let reply = engine.eval("let bad = n + \"x\"");
    assert_eq!(reply.status, "error");
    assert!(reply.message.contains("error"), "{}", reply.message);
    assert_eq!(reply.blob, "");
    // The session is unchanged: `bad` is unknown, `n` still works.
    assert_eq!(engine.eval("bad").status, "error");
    assert_eq!(engine.eval("n").status, "ok");
}

#[test]
fn a_parse_error_is_rendered_against_the_cell() {
    let mut engine = Engine::spawn();
    let reply = engine.eval("let = 3");
    assert_eq!(reply.status, "error");
    assert!(!reply.message.is_empty());
    assert_eq!(reply.blob, "");
}

#[test]
fn state_accumulates_across_cells() {
    let mut engine = Engine::spawn();
    assert_eq!(
        engine
            .eval("type Shape = Circle float | Rect float float")
            .status,
        "ok"
    );
    let reply = engine
        .eval("let area s =\n  match s:\n    case Circle r: 2.0 * r\n    case Rect w h: w * h");
    assert_eq!(reply.status, "ok");
    assert!(
        reply.message.starts_with("area : Shape -> float"),
        "{}",
        reply.message
    );
    let use_it = engine.eval("area (Rect 3.0 4.0)");
    assert_eq!(use_it.status, "ok");
    // The class definitions were executed with their own cells, not re-sent.
    assert!(!use_it.blob.contains("class Rect"), "{}", use_it.blob);
}

#[test]
fn an_expressions_infrastructure_is_remembered_but_its_statements_are_not() {
    let mut engine = Engine::spawn();
    // `Some 1` pulls in the Option classes (infrastructure) plus its own print.
    let first = engine.eval("Some 1");
    assert_eq!(first.status, "ok");
    assert!(first.blob.contains("class Some"), "{}", first.blob);
    let second = engine.eval("Some 1");
    assert!(
        !second.blob.contains("class Some"),
        "classes must not be re-sent"
    );
    assert!(second.blob.contains("print"), "{}", second.blob);
}

#[test]
fn type_op_answers_without_a_blob() {
    let mut engine = Engine::spawn();
    assert_eq!(engine.eval("let double x = x * 2").status, "ok");
    let reply = engine.request("type", "double");
    assert_eq!(reply.status, "ok");
    assert!(reply.message.starts_with("double : "), "{}", reply.message);
    assert_eq!(reply.blob, "");
}

#[test]
fn an_unknown_op_is_an_error_and_the_engine_survives() {
    let mut engine = Engine::spawn();
    let reply = engine.request("frobnicate", "x");
    assert_eq!(reply.status, "error");
    assert_eq!(engine.eval("1 + 1").status, "ok");
}

#[test]
fn a_cells_warnings_lead_its_output() {
    let mut engine = Engine::spawn();
    // An unused local in a definition: the warning, then the type echo.
    let reply = engine.eval("let f n =\n  let unused = 1\n  n + 1");
    assert_eq!(reply.status, "ok");
    assert!(
        reply.message.starts_with("warning: `unused` is never used"),
        "{}",
        reply.message
    );
    assert!(
        reply.message.ends_with("f : num a => a -> a\n"),
        "{}",
        reply.message
    );
    // An unreachable arm in a trailing expression.
    let reply = engine.eval("match 1:\n  case 1: \"a\"\n  case 1: \"b\"\n  case _: \"c\"");
    assert_eq!(reply.status, "ok");
    assert!(
        reply
            .message
            .contains("warning: this `case` can never match"),
        "{}",
        reply.message
    );
    // Binding a name again is how a notebook goes: not a warning here, and the
    // earlier cell's unused local is not repeated.
    let reply = engine.eval("let f n = n + 2");
    assert_eq!(reply.status, "ok");
    assert_eq!(reply.message, "f : num a => a -> a\n");
}

// ---------- Tracebacks in the cell's lines (`DESIGN.md` §9) ----------

/// Split an `eval-in` blob frame into its table and the Python to run.
fn table_and_blob(frame: &str) -> (&str, &str) {
    frame.split_once('\n').expect("a table line, then the blob")
}

#[test]
fn eval_in_returns_the_blobs_table_ahead_of_the_blob() {
    let mut engine = Engine::spawn();
    let reply = engine.request("eval-in", "In[4]\nlet ratio a b =\n    a // b\nratio 6 3");
    assert_eq!(reply.status, "ok", "{}", reply.message);
    let (table, blob) = table_and_blob(&reply.blob);
    // The blob is what `eval` would have returned.
    assert_eq!(
        blob,
        "def ratio(a, b):\n    return a // b\nprint(ratio(6, 3))\n"
    );
    // The table: the `def` line is the `let`, its body the second line of the
    // cell, and the expression that closes the cell is on the cell's line 3.
    assert_eq!(
        table,
        concat!(
            r#"{"source":"","rows":["#,
            r#"[1,1,["let ratio a b ="],"In[4]"],"#,
            r#"[2,2,["    a // b"],"In[4]"],"#,
            r#"[3,3,["ratio 6 3"],"In[4]"]"#,
            r#"]}"#
        )
    );
}

#[test]
fn a_later_cells_table_names_only_what_it_sends() {
    let mut engine = Engine::spawn();
    let first = engine.request("eval-in", "In[1]\nlet double x = x * 2");
    assert_eq!(first.status, "ok", "{}", first.message);
    // The second cell reuses `double`, already in the namespace: its blob and
    // its table hold the new statement alone, and a prelude helper it pulls in
    // is a row with no source.
    let second = engine.request("eval-in", "In[2]\n[1, 2] |> List.map double");
    assert_eq!(second.status, "ok", "{}", second.message);
    let (table, blob) = table_and_blob(&second.blob);
    assert!(!blob.contains("def double"), "{blob}");
    assert!(blob.starts_with("def _pf_map("), "{blob}");
    assert!(
        table.starts_with(r#"{"source":"","rows":[[1,0,[]],["#),
        "{table}"
    );
    assert!(
        table.ends_with(r#",["[1, 2] |> List.map double"],"In[2]"]]}"#),
        "{table}"
    );
    assert!(!table.contains("In[1]"), "{table}");
}

#[test]
fn plain_eval_keeps_its_three_frames_for_an_older_kernel() {
    let mut engine = Engine::spawn();
    let reply = engine.eval("let ratio a b = a // b\nratio 6 3");
    assert_eq!(reply.status, "ok", "{}", reply.message);
    assert_eq!(
        reply.blob,
        "def ratio(a, b):\n    return a // b\nprint(ratio(6, 3))\n"
    );
}

#[test]
fn the_engine_hands_out_the_traceback_hook() {
    let mut engine = Engine::spawn();
    let reply = engine.request("hook", "");
    assert_eq!(reply.status, "ok");
    assert!(reply.message.contains("def render("), "the hook's source");
    assert_eq!(reply.blob, "");
}

#[test]
fn a_rejected_cell_has_no_table() {
    let mut engine = Engine::spawn();
    let reply = engine.request("eval-in", "In[1]\nlet bad = 1 + True");
    assert_eq!(reply.status, "error");
    assert_eq!(reply.blob, "");
}
