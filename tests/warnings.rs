//! Warnings (`DESIGN.md` §3): the program compiles, and something in it does
//! nothing. Each test names the code that is warned about by the source text
//! under the warning's span, so a warning in the wrong place fails too.

use pyfun::diagnostics::WarningKind;

/// `(kind, the source text the warning points at)` for each warning in `src`.
fn warned(src: &str) -> Vec<(WarningKind, String)> {
    let analysis = pyfun::analyze(src);
    assert!(
        analysis.diagnostics.is_empty(),
        "the program should check: {:?}\n{src}",
        analysis.diagnostics
    );
    analysis
        .warnings
        .iter()
        .map(|w| (w.kind, src[w.span.start..w.span.end].to_string()))
        .collect()
}

/// The source text under each warning of `kind`.
fn of_kind(src: &str, kind: WarningKind) -> Vec<String> {
    warned(src)
        .into_iter()
        .filter(|(k, _)| *k == kind)
        .map(|(_, text)| text)
        .collect()
}

fn unreachable_arms(src: &str) -> Vec<String> {
    of_kind(src, WarningKind::UnreachableArm)
}

fn unused(src: &str) -> Vec<String> {
    of_kind(src, WarningKind::UnusedBinding)
}

const SHAPE: &str = "type Shape = Circle float | Square float | Dot\n";

#[test]
fn an_arm_the_arms_above_already_cover_is_unreachable() {
    // The same constructor twice, and a wildcard after every constructor.
    let src = format!(
        "{SHAPE}let area s =\n  match s:\n    case Circle r: r\n    case Square w: w\n    case Circle _: 0.0\n    case Dot: 0.0\n    case _: 1.0"
    );
    assert_eq!(unreachable_arms(&src), ["Circle _", "_"]);
    // A literal twice; the catch-all after it is still reachable.
    assert_eq!(
        unreachable_arms(
            "let f n =\n  match n:\n    case 0: \"zero\"\n    case 1: \"one\"\n    case 0: \"again\"\n    case _: \"other\""
        ),
        ["0"]
    );
    assert_eq!(
        unreachable_arms(
            "let f s =\n  match s:\n    case \"a\": 1\n    case \"a\": 2\n    case _: 3"
        ),
        ["\"a\""]
    );
    // Coverage is counted across columns: `(True, True)` is taken by the first
    // arm, and `(False, False)` is the one pair still open.
    assert_eq!(
        unreachable_arms(
            "let f p =\n  match p:\n    case (True, _): 1\n    case (_, True): 2\n    case (True, True): 3\n    case (False, False): 4"
        ),
        ["(True, True)"]
    );
    // Inside a constructor: `Some 3` after `Some x`.
    assert_eq!(
        unreachable_arms(
            "let f o =\n  match o:\n    case Some x: x\n    case None: 0\n    case Some 3: 3"
        ),
        ["Some 3"]
    );
    // After a name, nothing is left.
    assert_eq!(
        unreachable_arms("let f o =\n  match o:\n    case x: x\n    case 1: 1\n    case _: 0"),
        ["1", "_"]
    );
}

#[test]
fn unreachable_arms_are_found_through_records_lists_and_or_patterns() {
    // A record pattern with a name in every field takes every record.
    assert_eq!(
        unreachable_arms(
            "type P = { x: int, y: bool }\n\
             let f p =\n  match p:\n    case P { x = 0 }: 1\n    case P { x }: x\n    case P { y = True }: 2"
        ),
        ["P { y = True }"]
    );
    // A record-bodied constructor, by name.
    assert_eq!(
        unreachable_arms(
            "type S = Circle { r: int } | Dot\n\
             let f s =\n  match s:\n    case Circle {}: 1\n    case Circle { r = 0 }: 2\n    case Dot: 3"
        ),
        ["Circle { r = 0 }"]
    );
    // A list: empty and non-empty are everything.
    assert_eq!(
        unreachable_arms(
            "let f xs =\n  match xs:\n    case []: 0\n    case [x, *rest]: x\n    case [a, b]: a\n    case _: 1"
        ),
        ["[a, b]", "_"]
    );
    // An or-pattern is unreachable only when every alternative is.
    assert!(
        unreachable_arms(
            "let f n =\n  match n:\n    case 0: \"a\"\n    case 0 | 1: \"b\"\n    case _: \"c\""
        )
        .is_empty()
    );
    assert_eq!(
        unreachable_arms(
            "let f n =\n  match n:\n    case 0: \"a\"\n    case 1: \"b\"\n    case 0 | 1: \"c\"\n    case _: \"d\""
        ),
        ["0 | 1"]
    );
    // An as-pattern covers what its inner pattern does.
    assert_eq!(
        unreachable_arms(
            "let f o =\n  match o:\n    case Some x as whole: whole\n    case Some 1: o\n    case None: o"
        ),
        ["Some 1"]
    );
}

#[test]
fn a_guard_takes_nothing_for_certain() {
    // The guarded arm may decline, so the same pattern below it is reachable.
    assert!(
        unreachable_arms(
            "let f n =\n  match n:\n    case k if k > 10: \"big\"\n    case k: \"small\""
        )
        .is_empty()
    );
    assert!(
        unreachable_arms(
            "let f o =\n  match o:\n    case Some x if x > 0: x\n    case Some x: 0 - x\n    case None: 0"
        )
        .is_empty()
    );
    // A guarded arm is itself unreachable when the arms above take its pattern.
    assert_eq!(
        unreachable_arms(
            "let f o =\n  match o:\n    case Some x: x\n    case None: 0\n    case Some y if y > 3: y"
        ),
        ["Some y"]
    );
}

#[test]
fn an_ordinary_complete_match_has_no_unreachable_arm() {
    for src in [
        format!(
            "{SHAPE}let area s =\n  match s:\n    case Circle r: r\n    case Square w: w\n    case Dot: 0.0"
        ),
        "let f r =\n  match r:\n    case Ok (Some x): x\n    case Ok None: 0\n    case Error _: 0 - 1"
            .to_string(),
        "let f p =\n  match p:\n    case (0, _): 1\n    case (_, 0): 2\n    case (a, b): a + b"
            .to_string(),
        "let f xs =\n  match xs:\n    case []: 0\n    case [x]: x\n    case [x, y, *rest]: x + y"
            .to_string(),
        "let f b =\n  match b:\n    case True: 1\n    case False: 0".to_string(),
    ] {
        assert!(unreachable_arms(&src).is_empty(), "{src}");
    }
}

#[test]
fn two_kinds_of_match_are_left_unjudged() {
    // An active pattern's case is decided by running its recognizer: with an
    // argument, the same case name twice is two different tests.
    assert!(
        unreachable_arms(
            "let (|DivisibleBy|_|) d n = if n % d == 0 then Some (n // d) else None\n\
             let f n =\n  match n:\n    case DivisibleBy 3 q: q\n    case DivisibleBy 5 q: q\n    case _: 0"
        )
        .is_empty()
    );
    // A list pattern with elements after its star needs the exhaustiveness
    // check's cycle guard, so its match is not judged (and must not hang).
    assert!(
        unreachable_arms(
            "let f xs =\n  match xs:\n    case []: 0\n    case [*front, last]: last\n    case [*front, last]: last"
        )
        .is_empty()
    );
}

#[test]
fn a_local_let_nothing_reads_is_unused() {
    let src = "let total xs =\n  let unused = 5\n  let _kept = 6\n  let (a, b) = (1, 2)\n  let helper x = x + 1\n  let mut acc = 0\n  acc <- acc + List.len xs\n  acc + a";
    assert_eq!(unused(src), ["unused", "b", "helper"]);
    // In a computation expression, `let` and `let!` alike.
    assert_eq!(
        unused(
            "let g = result {\n  let! v = Ok 1\n  let w = 2\n  let! used = Ok 3\n  return used\n}"
        ),
        ["v", "w"]
    );
    // A use from a closure, an f-string or a nested block is a use.
    assert!(
        unused(
            "let f xs =\n  let k = 2\n  let label = \"n\"\n  let shown = f\"{label}\"\n  let inner =\n    let deep = k\n    deep + 1\n  xs |> List.map (fun x -> x * k + inner + String.len shown)"
        )
        .is_empty()
    );
    // A name shadowed before it is read is unused; the one that shadows it is not.
    assert_eq!(unused("let f n =\n  let x = n\n  let x = 2\n  x").len(), 1);
}

#[test]
fn what_is_bound_for_its_shape_is_not_reported() {
    // Parameters, pattern variables, loop targets and top-level names.
    assert!(
        unused(
            "let const x _ignored = 1\n\
             let first p =\n  match p:\n    case (a, b): a\n\
             let count xs = seq { for x in Seq.of_list xs: yield 1 } |> Seq.to_list |> List.len\n\
             let never_called = 3\n\
             let lam = fun a b -> a"
        )
        .is_empty()
    );
}

#[test]
fn a_typed_hole_holds_back_the_unused_binding_warning() {
    // The program is unfinished on purpose, and `time` is very likely what the
    // hole is waiting for.
    let src = "let speed dist =\n  let time = 2.0\n  dist / ?";
    let analysis = pyfun::analyze(src);
    assert_eq!(analysis.holes.len(), 1);
    assert!(analysis.warnings.is_empty(), "{:?}", analysis.warnings);
    // Filled in, a binding that is still unused is reported.
    assert_eq!(
        unused("let speed dist =\n  let time = 2.0\n  dist / 3.0"),
        ["time"]
    );
}

#[test]
fn an_extern_import_no_extern_is_rooted_at_is_unused() {
    let src = "extern import numpy as np\n\
               extern import math\n\
               extern import os.path\n\
               extern import json as js\n\
               extern root : float -> float = math.sqrt\n\
               extern dumps : int -> str = js.dumps";
    assert_eq!(
        of_kind(src, WarningKind::UnusedExternImport),
        ["extern import numpy as np", "extern import os.path"]
    );
    let message = &pyfun::analyze(src).warnings[0].message;
    assert_eq!(
        message,
        "`extern import numpy as np` is never used: no `extern` in this file is rooted at it"
    );
}

#[test]
fn a_top_level_name_bound_twice_is_reported_at_the_second() {
    let src = "let total = 1\nlet other = 2\nlet total = 3\nlet _scratch = 1\nlet _scratch = 2\nlet (a, other) = (1, 2)";
    let rebound = of_kind(src, WarningKind::Rebound);
    assert_eq!(rebound, ["total", "(a, other)"]);
    // A local that shadows a top-level name, or another local, is not this.
    assert!(
        of_kind(
            "let x = 1\nlet f n =\n  let x = n\n  let x = x + 1\n  x",
            WarningKind::Rebound
        )
        .is_empty()
    );
}

#[test]
fn warnings_wait_for_a_file_that_parses() {
    // A half-typed file has unused names everywhere; its errors are the news.
    let analysis =
        pyfun::analyze("let f n =\n  let unused = 1\n  n +\nlet total = 1\nlet total = 2");
    assert!(!analysis.parse_ok);
    assert!(analysis.warnings.is_empty());
    // A type error does not hide the warnings beside it.
    let analysis = pyfun::analyze("let f n =\n  let unused = 1\n  n + \"s\"");
    assert!(!analysis.diagnostics.is_empty());
    assert_eq!(analysis.warnings.len(), 1);
    // `pyfun::warnings`, which `run` and `compile` print, speaks only for a
    // program that builds.
    assert!(pyfun::warnings("let f n =\n  let unused = 1\n  n + \"s\"").is_empty());
    assert_eq!(
        pyfun::warnings("let f n =\n  let unused = 1\n  n + 1").len(),
        1
    );
}

#[test]
fn an_interactive_entry_shows_only_what_stands_on_its_own() {
    // A session binds a name again and imports ahead of use as it goes; only an
    // unreachable arm and an unused local are about one entry alone, and only
    // the newest entry's are shown.
    let defs = "let total = 1\nlet old n =\n  let stale = 1\n  n";
    let entry = "let total = 2\nlet f n =\n  let dead = 1\n  match n:\n    case 0: 0\n    case 0: 1\n    case _: 2";
    let combined = format!("{defs}\n{entry}");
    let analysis = pyfun::analyze(&combined);
    let shown: Vec<WarningKind> = pyfun::entry_warnings(&analysis, combined.len() - entry.len())
        .iter()
        .map(|w| w.kind)
        .collect();
    assert_eq!(
        shown,
        [WarningKind::UnusedBinding, WarningKind::UnreachableArm]
    );
    // The whole-file view has the older entry's unused local and the rebinding.
    assert_eq!(analysis.warnings.len(), 4);
}
