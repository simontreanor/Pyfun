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
