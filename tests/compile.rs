//! Phase 2 tests: lowering + Python emission.
//!
//! Two layers:
//! - String-level checks on the emitted Python (no interpreter needed).
//! - End-to-end execution: compile Pyfun, run the Python, assert on the result.
//!   These are skipped (not failed) when no `python`/`python3` is on PATH.

use std::io::Write;
use std::process::{Command, Stdio};

// ---------- string-level checks ----------

#[test]
fn curried_def_lowers_to_n_ary_def() {
    let py = pyfun::compile("let add a b = a + b").unwrap();
    assert!(py.contains("def add(a, b):"), "{py}");
    assert!(py.contains("return a + b"), "{py}");
}

#[test]
fn full_application_is_a_direct_call() {
    let py = pyfun::compile("let add a b = a + b\nlet r = add 1 2").unwrap();
    assert!(py.contains("r = add(1, 2)"), "{py}");
}

#[test]
fn unit_literal_lowers_to_none() {
    let py = pyfun::compile("let nothing = ()").unwrap();
    assert!(py.contains("nothing = None"), "{py}");
}

#[test]
fn partial_application_uses_functools_partial() {
    let py = pyfun::compile("let add a b = a + b\nlet inc = add 1").unwrap();
    assert!(py.starts_with("import functools\n"), "{py}");
    assert!(py.contains("inc = functools.partial(add, 1)"), "{py}");
}

#[test]
fn no_functools_import_when_unused() {
    let py = pyfun::compile("let add a b = a + b\nlet r = add 1 2").unwrap();
    assert!(!py.contains("import functools"), "{py}");
}

#[test]
fn extern_lowers_to_its_python_target_with_import() {
    let py = pyfun::compile("extern pure tan: float -> float = math.tan\nlet r = tan 8.0").unwrap();
    assert!(py.contains("import math"), "{py}");
    assert!(py.contains("r = math.tan(8.0)"), "{py}");
}

#[test]
fn extern_with_name_equal_target_needs_no_import() {
    let py = pyfun::compile("extern show: a -> string = str\nlet r = show 42").unwrap();
    assert!(!py.contains("import"), "{py}");
    assert!(py.contains("r = str(42)"), "{py}");
}

#[test]
fn partial_application_of_extern_uses_functools_partial() {
    let py =
        pyfun::compile("extern pow: float -> float -> float = math.pow\nlet sq = pow 2.0").unwrap();
    assert!(py.contains("sq = functools.partial(math.pow, 2.0)"), "{py}");
}

#[test]
fn unused_extern_imports_nothing() {
    let py = pyfun::compile("extern pure tan: float -> float = math.tan\nlet r = 1").unwrap();
    assert!(!py.contains("import math"), "{py}");
}

#[test]
fn extern_in_submodule_imports_the_submodule() {
    // A target inside a submodule must import the submodule, not just the top-level
    // package — `import urllib` would leave `urllib.parse` unbound at runtime.
    let py = pyfun::compile("extern q: string -> string = urllib.parse.quote\nlet r = q \"a b\"")
        .unwrap();
    assert!(py.contains("import urllib.parse"), "{py}");
    assert!(py.contains("r = urllib.parse.quote("), "{py}");
}

#[test]
fn extern_type_declares_an_opaque_handle_and_emits_no_class() {
    // `extern type` registers a nominal type usable in extern signatures but erases
    // at lowering — no Python class, no constructor.
    let py = pyfun::compile(
        "extern type Conn\nextern connect: string -> Conn = sqlite3.connect\nlet db = connect \":memory:\"",
    )
    .unwrap();
    assert!(py.contains("db = sqlite3.connect(\":memory:\")"), "{py}");
    assert!(!py.contains("class Conn"), "{py}");
    assert!(!py.contains("ConnH"), "{py}");
}

#[test]
fn extern_type_replaces_the_phantom_adt_in_the_instance_extern_pattern() {
    // The opaque-handle idiom works end-to-end with instance-method externs.
    let py = pyfun::compile(
        "extern type C\nextern ex: C -> string -> C = .execute()\nlet f c = ex c \"select 1\"",
    )
    .unwrap();
    assert!(py.contains("c.execute(\"select 1\")"), "{py}");
    assert!(!py.contains("class C"), "{py}");
}

#[test]
fn instance_method_extern_lowers_to_a_method_call() {
    // `= .read()` calls the method on the first argument (the receiver).
    let py =
        pyfun::compile("type R = RH\nextern readBody: R -> string = .read()\nlet f r = readBody r")
            .unwrap();
    assert!(py.contains("return r.read()"), "{py}");
}

#[test]
fn instance_method_extern_passes_remaining_args() {
    let py = pyfun::compile(
        "type C = CH\nextern ex: C -> string -> C = .execute()\nlet f c = ex c \"select 1\"",
    )
    .unwrap();
    assert!(py.contains("c.execute(\"select 1\")"), "{py}");
}

#[test]
fn instance_method_receiver_only_partial_is_the_bound_method() {
    // Applying just the receiver yields the Python bound method — the curried
    // partial with no `functools.partial` wrapper.
    let py =
        pyfun::compile("type C = CH\nextern ex: C -> string -> C = .execute()\nlet g c = ex c")
            .unwrap();
    assert!(py.contains("return c.execute"), "{py}");
    assert!(!py.contains("functools.partial"), "{py}");
}

#[test]
fn instance_property_extern_reads_the_attribute() {
    // `= .scheme` (no `()`) reads the attribute — no call.
    let py =
        pyfun::compile("type U = UH\nextern scheme: U -> string = .scheme\nlet f u = scheme u")
            .unwrap();
    assert!(py.contains("return u.scheme"), "{py}");
    assert!(!py.contains("u.scheme("), "{py}");
}

#[test]
fn nullary_extern_lowers_to_a_zero_arg_call() {
    // `unit -> a` applied to `()` is a zero-argument Python call, not `f(None)`.
    let py = pyfun::compile("extern now: unit -> float = time.time\nlet t = now ()").unwrap();
    assert!(py.contains("t = time.time()"), "{py}");
    assert!(!py.contains("time.time(None)"), "{py}");
    assert!(py.contains("import time"), "{py}");
}

#[test]
fn extern_on_builtin_type_imports_nothing() {
    // A dotted target rooted at a builtin type is always in scope — no `import`.
    let py = pyfun::compile("extern up: string -> string = str.upper\nlet r = up \"hi\"").unwrap();
    assert!(py.contains("r = str.upper(\"hi\")"), "{py}");
    assert!(!py.contains("import str"), "{py}");
}

#[test]
fn extern_on_class_method_imports_only_the_module() {
    // A capitalized segment is a class attribute, not a submodule, so the import
    // stops before it: `sqlite3.Connection.execute` imports `sqlite3`, not
    // `sqlite3.Connection` (which is not importable).
    let py = pyfun::compile(
        "type C = CH\nextern ex: C -> string -> C = sqlite3.Connection.execute\nlet f c = ex c \"x\"",
    )
    .unwrap();
    assert!(
        py.contains("import sqlite3\n") || py.contains("import sqlite3\r"),
        "{py}"
    );
    assert!(!py.contains("import sqlite3.Connection"), "{py}");
}

#[test]
fn extern_import_declares_the_module_explicitly() {
    // `extern import datetime` declares the module boundary: import it as
    // written, reference the full dotted path. Without the declaration the
    // lowercase-prefix heuristic would wrongly emit `import datetime.datetime`.
    let py = pyfun::compile(
        "extern import datetime\nextern now: unit -> a = datetime.datetime.now\nlet t = now ()",
    )
    .unwrap();
    assert!(
        py.contains("import datetime\n") || py.contains("import datetime\r"),
        "{py}"
    );
    assert!(!py.contains("import datetime.datetime"), "{py}");
    assert!(py.contains("t = datetime.datetime.now()"), "{py}");
}

#[test]
fn extern_import_covers_a_fully_applied_value_attribute_target() {
    // `extern import sys` roots `sys.stdout.write` (a value attribute the
    // heuristic would read as a submodule); the call goes through the full path.
    let py = pyfun::compile(
        "extern import sys\nextern write: string -> int = sys.stdout.write\nlet r = write \"hi\"",
    )
    .unwrap();
    assert!(
        py.contains("import sys\n") || py.contains("import sys\r"),
        "{py}"
    );
    assert!(!py.contains("import sys.stdout"), "{py}");
    assert!(py.contains("r = sys.stdout.write(\"hi\")"), "{py}");
}

#[test]
fn bare_reference_to_a_declared_import_extern_still_records_it() {
    // A bare (unapplied) reference must still record the declared module — the
    // reference lowers to the dotted path, which needs `sys` bound.
    let py = pyfun::compile(
        "extern import sys\nextern write: string -> int = sys.stdout.write\nlet w = write",
    )
    .unwrap();
    assert!(
        py.contains("import sys\n") || py.contains("import sys\r"),
        "{py}"
    );
    assert!(py.contains("w = sys.stdout.write"), "{py}");
}

#[test]
fn extern_import_alias_emits_import_as_and_roots_targets() {
    // `extern import json as j` mirrors Python's aliased import: the emitted
    // import carries the alias, and targets root at the alias name.
    let py = pyfun::compile(
        "extern import json as j\nextern dumps: List int -> string = j.dumps\nlet s = dumps [1]",
    )
    .unwrap();
    assert!(py.contains("import json as j"), "{py}");
    assert!(py.contains("s = j.dumps([1])"), "{py}");
}

#[test]
fn extern_import_composes_with_pinned_kwargs() {
    // A declared import and pinned kwargs coexist: import the declared module
    // and append the kwargs to the emitted call as usual.
    let py = pyfun::compile(
        "extern import sqlite3\nextern conn: string -> a = sqlite3.dbapi2.connect(timeout=5)\nlet c = conn \":memory:\"",
    )
    .unwrap();
    assert!(
        py.contains("import sqlite3\n") || py.contains("import sqlite3\r"),
        "{py}"
    );
    assert!(!py.contains("import sqlite3.dbapi2"), "{py}");
    assert!(
        py.contains("c = sqlite3.dbapi2.connect(\":memory:\", timeout=5)"),
        "{py}"
    );
}

#[test]
fn import_heuristic_is_unchanged_without_a_declared_import() {
    // With no `extern import`, the lowercase-prefix heuristic is untouched: a
    // submodule target still imports the submodule, not just the top package.
    let py = pyfun::compile("extern get: string -> a = urllib.request.urlopen\nlet r = get \"x\"")
        .unwrap();
    assert!(py.contains("import urllib.request"), "{py}");
    assert!(py.contains("r = urllib.request.urlopen(\"x\")"), "{py}");
}

#[test]
fn unused_extern_import_emits_nothing() {
    // A declared module import is hoisted only when a target rooted at it is
    // actually used — an unused declaration leaves no trace in the output.
    let py = pyfun::compile("extern import sqlite3\nlet x = 1").unwrap();
    assert!(!py.contains("import sqlite3"), "{py}");
}

#[test]
fn extern_kwargs_are_appended_to_the_call() {
    // A plain extern with pinned kwargs appends them to every call.
    let py = pyfun::compile(
        "extern openText : string -> Seq string = builtins.open(mode=\"rt\", encoding=\"utf-8\")\n\
         let f path = openText path",
    )
    .unwrap();
    assert!(
        py.contains("return builtins.open(path, mode=\"rt\", encoding=\"utf-8\")"),
        "{py}"
    );
}

#[test]
fn receiver_method_extern_kwargs_follow_the_positional_args() {
    // A `= .method(kw=v)` receiver-method extern appends its kwargs after the
    // positional method arguments.
    let py = pyfun::compile(
        "type P = PH\n\
         extern writeText : P -> string -> int = .write_text(encoding=\"utf-8\")\n\
         let f p text = writeText p text",
    )
    .unwrap();
    assert!(
        py.contains("return p.write_text(text, encoding=\"utf-8\")"),
        "{py}"
    );
}

#[test]
fn extern_kwargs_support_int_negative_bool_and_float_values() {
    let py = pyfun::compile(
        "extern g : string -> a = gzip.open(compresslevel=9, buffering=-1, text=true, ratio=2.5)\n\
         let f path = g path",
    )
    .unwrap();
    assert!(
        py.contains("gzip.open(path, compresslevel=9, buffering=-1, text=True, ratio=2.5)"),
        "{py}"
    );
}

#[test]
fn extern_kwargs_partial_application_pins_kwargs_via_functools_partial() {
    // Under-application (partial or bare) must NOT drop the pinned kwargs: they
    // ride along on `functools.partial`, so a later application still supplies them.
    let py = pyfun::compile(
        "extern openText : string -> int -> Seq string = builtins.open(mode=\"rt\")\n\
         let bare = openText\n\
         let partial = openText \"a.txt\"",
    )
    .unwrap();
    assert!(
        py.contains("bare = functools.partial(builtins.open, mode=\"rt\")"),
        "{py}"
    );
    assert!(
        py.contains("partial = functools.partial(builtins.open, \"a.txt\", mode=\"rt\")"),
        "{py}"
    );
}

#[test]
fn e2e_extern_kwargs_produce_observable_output() {
    // `int` takes a real Python kwarg (`base`); pinning `base=16` proves the kwargs
    // reach the live call — `parseHex "ff"` evaluates to 255, not a parse error.
    run_and_check(
        "extern parseHex : string -> int = int(base=16)\n\
         let n = parseHex \"ff\"\n\
         let m = parseHex \"10\"",
        &[("n", "255"), ("m", "16")],
    );
}

#[test]
fn list_literal_lowers_to_a_python_list() {
    let py = pyfun::compile("let xs = [1, 2, 3]").unwrap();
    assert!(py.contains("xs = [1, 2, 3]"), "{py}");
}

#[test]
fn map_filter_lower_to_emitted_helpers() {
    let py = pyfun::compile("let r = List.map (fun x -> x) (List.filter (fun x -> true) [1, 2])")
        .unwrap();
    assert!(py.contains("def _pf_map(f, xs):"), "{py}");
    assert!(py.contains("return list(map(f, xs))"), "{py}");
    assert!(py.contains("def _pf_filter(f, xs):"), "{py}");
    assert!(py.contains("return list(filter(f, xs))"), "{py}");
}

#[test]
fn len_and_sum_lower_to_python_builtins_without_helpers() {
    let py = pyfun::compile("let n = List.len [1, 2]\nlet s = List.sum [1, 2]").unwrap();
    assert!(py.contains("n = len([1, 2])"), "{py}");
    assert!(py.contains("s = sum([1, 2])"), "{py}");
    assert!(!py.contains("_pf_"), "no helpers needed: {py}");
}

#[test]
fn fold_lowers_to_functools_reduce() {
    let py = pyfun::compile("let t = List.fold (fun a b -> a + b) 0 [1, 2, 3]").unwrap();
    assert!(py.starts_with("import functools\n"), "{py}");
    assert!(py.contains("def _pf_fold(f, acc, xs):"), "{py}");
    assert!(py.contains("return functools.reduce(f, xs, acc)"), "{py}");
}

#[test]
fn unused_list_helpers_are_not_emitted() {
    let py = pyfun::compile("let xs = [1, 2, 3]").unwrap();
    assert!(!py.contains("_pf_"), "{py}");
}

#[test]
fn pipe_becomes_application() {
    let py = pyfun::compile("let id x = x\nlet r = 5 |> id").unwrap();
    assert!(py.contains("r = id(5)"), "{py}");
}

#[test]
fn empty_collections_lower_to_set_and_dict() {
    let py = pyfun::compile("let s = Set.empty\nlet m = Map.empty").unwrap();
    assert!(py.contains("s = set()"), "{py}");
    assert!(py.contains("m = dict()"), "{py}");
}

#[test]
fn set_functions_lower_to_emitted_helpers() {
    let py = pyfun::compile("let r = Set.contains 1 (Set.add 1 (Set.ofList [2]))").unwrap();
    assert!(py.contains("def _pf_set_add(x, s):"), "{py}");
    assert!(py.contains("return s.union([x])"), "{py}");
    // `Set.contains` is a fully-applied pure predicate, so it inlines to `x in s`
    // (Lever A) rather than emitting the `_pf_set_contains` helper.
    assert!(py.contains("r = 1 in _pf_set_add(1, set([2]))"), "{py}");
    assert!(!py.contains("_pf_set_contains"), "contains inlined: {py}");
    // `Set.ofList` is a bare builtin, no helper.
    assert!(py.contains("set([2])"), "{py}");
}

#[test]
fn map_add_and_find_or_lower_to_emitted_helpers() {
    let py =
        pyfun::compile("let m = Map.add \"a\" 1 Map.empty\nlet v = Map.findOr \"a\" 0 m").unwrap();
    assert!(py.contains("def _pf_map_add(k, v, m):"), "{py}");
    assert!(
        py.contains("return dict(list(m.items()) + [[k, v]])"),
        "{py}"
    );
    assert!(py.contains("def _pf_map_find_or(k, default, m):"), "{py}");
    assert!(py.contains("return m.get(k, default)"), "{py}");
}

#[test]
fn map_remove_lowers_to_a_copy_and_pop() {
    let py = pyfun::compile("let m = Map.remove \"a\" Map.empty").unwrap();
    assert!(py.contains("def _pf_map_remove(k, m):"), "{py}");
    assert!(py.contains("r = dict(m)"), "{py}");
    assert!(py.contains("r.pop(k, None)"), "{py}");
    assert!(py.contains("return r"), "{py}");
}

#[test]
fn try_find_lowers_to_an_option_and_pulls_in_the_option_prelude() {
    let py = pyfun::compile("let v = Map.tryFind \"a\" Map.empty").unwrap();
    assert!(py.contains("class Some:"), "{py}");
    assert!(py.contains("class None_:"), "{py}");
    assert!(py.contains("def _pf_map_try_find(k, m):"), "{py}");
    assert!(py.contains("return Some(m.get(k))"), "{py}");
    assert!(py.contains("return None_()"), "{py}");
}

#[test]
fn list_zip_lowers_to_a_zip_helper() {
    let py = pyfun::compile("let ps = List.zip [1, 2] [3, 4]").unwrap();
    assert!(py.contains("def _pf_zip(xs, ys):"), "{py}");
    assert!(py.contains("return list(zip(xs, ys))"), "{py}");
}

#[test]
fn map_of_list_lowers_to_dict_and_to_list_to_items() {
    // `Map.ofList` is a bare `dict` over the pair list; `Map.toList` is a helper.
    let py = pyfun::compile("let m = Map.ofList [(1, 2)]\nlet ps = Map.toList m").unwrap();
    assert!(py.contains("m = dict([(1, 2)])"), "{py}");
    assert!(py.contains("def _pf_map_to_list(m):"), "{py}");
    assert!(py.contains("return list(m.items())"), "{py}");
}

#[test]
fn e2e_zip_into_a_map_and_back() {
    run_and_check(
        "
        let m = Map.ofList (List.zip [\"a\", \"b\"] [1, 2])
        let a = Option.withDefault 0 (Map.tryFind \"a\" m)
        let n = Map.len m
        let pairs = Map.toList m
        ",
        &[("a", "1"), ("n", "2"), ("pairs", "[('a', 1), ('b', 2)]")],
    );
}

#[test]
fn try_lowers_to_a_try_except_yielding_ok_or_error() {
    let py = pyfun::compile(
        "extern parseInt : string -> int = int\n\
         let safe s = try (parseInt s)",
    )
    .unwrap();
    // The Exception record is emitted as `_Exception` (not shadowing the builtin).
    assert!(py.contains("class _Exception:"), "{py}");
    assert!(py.contains("try:"), "{py}");
    assert!(py.contains("= Ok(int(s))"), "{py}");
    assert!(py.contains("except Exception as"), "{py}");
    // The handler builds Error(_Exception(type(e).__name__, str(e))).
    assert!(py.contains("_Exception(type("), "{py}");
    assert!(py.contains(").__name__, str("), "{py}");
}

#[test]
fn e2e_try_catches_and_recovers() {
    run_and_check(
        "
        extern parseInt : string -> int = int
        let orZero s = Result.withDefault 0 (try (parseInt s))
        let good = orZero \"42\"
        let bad = orZero \"oops\"
        ",
        &[("good", "42"), ("bad", "0")],
    );
}

#[test]
fn e2e_try_exposes_exception_kind_and_message() {
    run_and_check(
        "
        extern parseInt : string -> int = int
        let kindOf s =
          match try (parseInt s):
            case Ok n: \"ok\"
            case Error e: e.errorKind
        let k = kindOf \"nope\"
        let matched =
          match try (parseInt \"x\"):
            case Error (Exception { errorKind = \"ValueError\" }): \"caught-value-error\"
            case _: \"other\"
        ",
        &[("k", "ValueError"), ("matched", "caught-value-error")],
    );
}

#[test]
fn interpolated_string_lowers_to_a_python_fstring() {
    let py = pyfun::compile(
        "let name = \"Ada\"\n\
         let a = 3\n\
         let b = 4\n\
         let g = f\"hi {name}: {a + b}\"\n\
         let u = f\"upper {String.upper name}\"\n\
         let e = f\"brace {{ {a}\"",
    )
    .unwrap();
    assert!(py.contains("g = f\"hi {name}: {a + b}\""), "{py}");
    // A module member in a hole lowers to its helper call, inside the f-string.
    assert!(py.contains("u = f\"upper {_pf_str_upper(name)}\""), "{py}");
    // `{{` stays a literal brace in the emitted f-string.
    assert!(py.contains("e = f\"brace {{ {a}\""), "{py}");
}

#[test]
fn e2e_interpolated_strings() {
    run_and_check(
        "
        let name = \"Ada\"
        let a = 3
        let b = 4
        let p = (1, 2)
        let greet = f\"Hello, {name}!\"
        let math = f\"{a} + {b} = {a + b}\"
        let up = f\"upper: {String.upper name}\"
        let brace = f\"{{literal}} {a}\"
        let point = f\"point {p}\"
        ",
        &[
            ("greet", "Hello, Ada!"),
            ("math", "3 + 4 = 7"),
            ("up", "upper: ADA"),
            ("brace", "{literal} 3"),
            ("point", "point (1, 2)"),
        ],
    );
}

#[test]
fn debug_hole_lowers_to_an_echoed_literal_plus_hole() {
    // `{x=}` resolves at lex time to a literal chunk (the raw hole text incl. the
    // `=`) followed by an ordinary hole, so the emitted Python is `f"x={x}"`.
    let py = pyfun::compile(
        "let x = 3\n\
         let y = 4\n\
         let a = f\"{x=}\"\n\
         let b = f\"{x = }\"\n\
         let c = f\"sum {x + y=} end\"\n\
         let d = f\"{x==y}\"",
    )
    .unwrap();
    assert!(py.contains("a = f\"x={x}\""), "{py}");
    // Whitespace around the marker is echoed verbatim.
    assert!(py.contains("b = f\"x = {x}\""), "{py}");
    assert!(py.contains("c = f\"sum x + y={x + y} end\""), "{py}");
    // A trailing `==` is a comparison, not a debug marker: nothing is echoed.
    assert!(py.contains("d = f\"{x == y}\""), "{py}");
}

#[test]
fn e2e_mutual_recursion() {
    // Mutually-recursive functions lower to plain Python defs (which resolve names
    // at call time, so no reordering is needed) and run.
    run_and_check(
        "
        let isEven = fun n -> if n == 0 then true else isOdd (n - 1)
        let isOdd = fun n -> if n == 0 then false else isEven (n - 1)
        let a = isEven 10
        let b = isOdd 10
        ",
        &[("a", "True"), ("b", "False")],
    );
}

#[test]
fn as_pattern_lowers_to_python_as() {
    let py = pyfun::compile("let f o =\n  match o:\n    case Some v as w: w\n    case None: None")
        .unwrap();
    assert!(py.contains("case Some(v) as w:"), "{py}");
}

#[test]
fn e2e_as_pattern() {
    run_and_check(
        "
        let describe = fun n ->
            match n:
                case 0: 0
                case x as v: v
        let both = fun p ->
            match p:
                case (a, b) as w: w
        let a = describe 7
        let b = both (3, 4)
        ",
        &[("a", "7"), ("b", "(3, 4)")],
    );
}

#[test]
fn discard_binding_lowers_and_runs_effects() {
    // `let _ = e` lowers to `_ = e` (Python's idiomatic discard); the effect runs.
    let py = pyfun::compile("let _ = 1 + 2").unwrap();
    assert!(py.contains("_ = 1 + 2"), "{py}");
}

#[test]
fn e2e_discard_runs_the_effect() {
    // A discarded `print` still executes, and a following statement runs too.
    run_and_check(
        "
        let go =
            let _ = 1 + 2
            42
        ",
        &[("go", "42")],
    );
}

#[test]
fn string_slice_lowers_to_python_slicing() {
    let py = pyfun::compile("let a = String.slice 0 3 \"hello\"").unwrap();
    assert!(py.contains("return s[start:end]"), "readable slice: {py}");
}

#[test]
fn e2e_string_slice_and_index_of() {
    run_and_check(
        "
        let s = \"hello world\"
        let a = String.slice 0 5 s
        let b = String.slice 6 100 s
        let c = Option.withDefault (0 - 1) (String.tryIndexOf \"world\" s)
        let d = Option.withDefault (0 - 1) (String.tryIndexOf \"zzz\" s)
        ",
        &[
            ("a", "hello"),
            ("b", "world"), // out-of-range end clamps (total)
            ("c", "6"),
            ("d", "-1"), // not found -> None -> default
        ],
    );
}

// ---------- Lever A: inline fully-applied pure stdlib predicates ----------

#[test]
fn stdlib_predicates_inline_when_fully_applied() {
    let py = pyfun::compile(
        "let a s = String.contains \"x\" s
         let b s = String.startsWith \"x\" s
         let c s = String.endsWith \"x\" s
         let d xs = List.contains 3 xs
         let e s = Set.contains 3 s
         let g m = Map.contains 3 m
         let h xs = List.isEmpty xs",
    )
    .unwrap();
    // The idiom is emitted directly, not a `_pf_*` helper call. Argument order
    // matches each helper body (`sub in s`, `s.startswith(pre)`, …).
    assert!(py.contains("return \"x\" in s"), "String.contains: {py}");
    assert!(
        py.contains("return s.startswith(\"x\")"),
        "startsWith: {py}"
    );
    assert!(py.contains("return s.endswith(\"x\")"), "endsWith: {py}");
    assert!(py.contains("return 3 in xs"), "List.contains: {py}");
    assert!(py.contains("return 3 in s"), "Set.contains: {py}");
    assert!(py.contains("return 3 in m"), "Map.contains: {py}");
    assert!(py.contains("return not xs"), "List.isEmpty: {py}");
    // None of the inlined helpers are defined, since nothing references them.
    assert!(!py.contains("_pf_str_contains"), "no contains helper: {py}");
    assert!(
        !py.contains("_pf_str_starts_with"),
        "no startsWith helper: {py}"
    );
    assert!(
        !py.contains("_pf_str_ends_with"),
        "no endsWith helper: {py}"
    );
    assert!(
        !py.contains("_pf_list_contains"),
        "no list-contains helper: {py}"
    );
    assert!(
        !py.contains("_pf_set_contains"),
        "no set-contains helper: {py}"
    );
    assert!(
        !py.contains("_pf_map_contains"),
        "no map-contains helper: {py}"
    );
    assert!(!py.contains("_pf_is_empty"), "no isEmpty helper: {py}");
}

#[test]
fn inlined_membership_parenthesizes_correctly() {
    // `and` binds looser than `in`, so no parens; `not` is looser still.
    let py = pyfun::compile(
        "let f s = String.contains \"a\" s and String.contains \"b\" s
         let g s = if String.contains \"a\" s then 1 else 0",
    )
    .unwrap();
    assert!(py.contains("return \"a\" in s and \"b\" in s"), "and: {py}");
    assert!(py.contains("if \"a\" in s:"), "if-guard: {py}");
}

#[test]
fn stdlib_predicate_partial_application_falls_back_to_helper() {
    // A bare partial (one arg of two) is NOT inlined — it still routes through the
    // `_pf_str_contains` helper via `functools.partial`, so `List.map`/`filter`
    // over the partial keeps working.
    let py = pyfun::compile(
        "let f = String.contains \"x\"
         let r = List.filter f [\"xy\", \"ab\"]",
    )
    .unwrap();
    assert!(
        py.contains("def _pf_str_contains(sub, s):"),
        "helper defined: {py}"
    );
    assert!(
        py.contains("f = functools.partial(_pf_str_contains, \"x\")"),
        "partial routes to helper: {py}"
    );
}

#[test]
fn e2e_inlined_stdlib_predicates() {
    // Value + argument-order correctness of every inlined idiom.
    run_and_check(
        "
        let a = String.contains \"lo\" \"hello\"
        let b = String.contains \"hello\" \"lo\"
        let c = String.startsWith \"he\" \"hello\"
        let d = String.startsWith \"lo\" \"hello\"
        let e = String.endsWith \"lo\" \"hello\"
        let f = String.endsWith \"he\" \"hello\"
        let g = List.contains 3 [1, 2, 3]
        let h = List.contains 9 [1, 2, 3]
        let i = Set.contains 2 (Set.ofList [1, 2, 3])
        let j = Map.contains 1 (Map.ofList [(1, \"a\")])
        let k = Map.contains 9 (Map.ofList [(1, \"a\")])
        let l = List.isEmpty (List.filter (fun x -> x > 9) [1, 2, 3])
        let m = List.isEmpty [1, 2]
        ",
        &[
            ("a", "True"),
            ("b", "False"),
            ("c", "True"),
            ("d", "False"),
            ("e", "True"),
            ("f", "False"),
            ("g", "True"),
            ("h", "False"),
            ("i", "True"),
            ("j", "True"),
            ("k", "False"),
            ("l", "True"),
            ("m", "False"),
        ],
    );
}

#[test]
fn e2e_stdlib_predicate_partial_application() {
    // The fallback path computes the right value: a partial applied later, and a
    // partial handed to `List.filter`, both go through the helper.
    run_and_check(
        "
        let f = String.contains \"lo\"
        let a = f \"hello\"
        let b = f \"hi\"
        let r = List.filter (String.contains \"a\") [\"cat\", \"dog\", \"bat\"]
        let n = List.len r
        ",
        &[("a", "True"), ("b", "False"), ("n", "2")],
    );
}

#[test]
fn exponentiation_lowers_right_assoc() {
    let py = pyfun::compile("let a = 2.0 ** 3.0 ** 2.0\nlet b = -2.0 ** 2.0").unwrap();
    // Right-associative, so no parens needed on the nested `**`.
    assert!(py.contains("a = 2.0 ** 3.0 ** 2.0"), "{py}");
    // `**` binds tighter than unary minus: `-2.0 ** 2.0` is `-(2.0 ** 2.0)`.
    assert!(py.contains("b = -2.0 ** 2.0"), "{py}");
}

#[test]
fn e2e_exponentiation() {
    run_and_check(
        "
        let a = 2.0 ** 8.0
        let b = 2.0 ** 3.0 ** 2.0
        let c = 2.0 ** -1.0
        let d = -2.0 ** 2.0
        ",
        // b: 2^(3^2)=2^9=512; d: -(2^2)=-4.
        &[("a", "256.0"), ("b", "512.0"), ("c", "0.5"), ("d", "-4.0")],
    );
}

#[test]
fn e2e_option_bind_filter_to_result() {
    run_and_check(
        "
        let half = fun x -> if x % 2 == 0 then Some (x // 2) else None
        let a = Option.withDefault 0 (Option.bind half (Some 8))
        let b = Option.withDefault 0 (Option.bind half (Some 7))
        let c = Option.withDefault 0 (Option.filter (fun x -> x > 5) (Some 8))
        let d = Option.withDefault 0 (Option.filter (fun x -> x > 5) (Some 2))
        let e = Result.withDefault 0 (Option.toResult 0 (Some 42))
        let f = Result.isError (Option.toResult 0 None)
        ",
        &[
            ("a", "4"),
            ("b", "0"),
            ("c", "8"),
            ("d", "0"),
            ("e", "42"),
            ("f", "True"),
        ],
    );
}

#[test]
fn numeric_conversions_lower_correctly() {
    let py = pyfun::compile(
        "let a = round 3.7\n\
         let b = floor 3.2\n\
         let c = ceil 3.2\n\
         let d = truncate 3.9",
    )
    .unwrap();
    assert!(py.contains("import math"), "{py}");
    assert!(
        py.contains("a = round(3.7)"),
        "round is a bare builtin: {py}"
    );
    assert!(py.contains("b = math.floor(3.2)"), "{py}");
    assert!(py.contains("c = math.ceil(3.2)"), "{py}");
    assert!(
        py.contains("d = math.trunc(3.9)"),
        "truncate -> math.trunc: {py}"
    );
}

#[test]
fn sqrt_lowers_to_math_sqrt_with_units_erased() {
    // The unit-aware prelude sqrt (`float<'u^2> -> float<'u>`) lowers to
    // `math.sqrt` with the unit annotation fully erased.
    let py = pyfun::compile("measure m\nlet side = sqrt 16.0<m^2>").unwrap();
    assert!(py.contains("import math"), "{py}");
    assert!(py.contains("side = math.sqrt(16.0)"), "units erased: {py}");
    // A bare reference is the attribute itself (first-class value).
    let py = pyfun::compile("let f = sqrt\nlet r = f 4.0").unwrap();
    assert!(py.contains("f = math.sqrt"), "{py}");
}

#[test]
fn a_user_sqrt_shadows_the_builtin_in_lowering() {
    let py = pyfun::compile("let sqrt x = x\nlet r = sqrt 3").unwrap();
    assert!(py.contains("def sqrt(x):"), "{py}");
    assert!(!py.contains("math.sqrt"), "user def must win: {py}");
}

#[test]
fn e2e_unit_aware_sqrt() {
    // √(16 m²) = 4 m — the number is right and the unit is gone at runtime.
    run_and_check(
        "measure m\n\
         let side = sqrt 16.0<m^2>\n\
         let hyp = sqrt (2.0 * 2.0 + 2.0 * 2.0)",
        &[("side", "4.0"), ("hyp", "2.8284271247461903")],
    );
}

#[test]
fn cbrt_lowers_to_math_cbrt_with_units_erased() {
    let py = pyfun::compile("measure m\nlet side = cbrt 27.0<m^3>").unwrap();
    assert!(py.contains("import math"), "{py}");
    assert!(py.contains("side = math.cbrt(27.0)"), "units erased: {py}");
    let py = pyfun::compile("let f = cbrt\nlet r = f 8.0").unwrap();
    assert!(py.contains("f = math.cbrt"), "{py}");
}

#[test]
fn e2e_unit_aware_cbrt() {
    // ∛(27 m³) = 3 m, and `math.cbrt` cube-roots negatives correctly (unlike `**`).
    run_and_check(
        "measure m\n\
         let side = cbrt 27.0<m^3>\n\
         let neg = cbrt (0.0 - 8.0)",
        &[("side", "3.0"), ("neg", "-2.0")],
    );
}

/// The showcase example must always type-check and lower — a guard so a change
/// like reserving a new builtin (which can clash with an `extern` name in the
/// example) can't silently break it. Always runs (no interpreter needed).
#[test]
fn the_hello_example_type_checks_and_compiles() {
    let src = include_str!("../examples/hello.pyfun");
    pyfun::compile(src).expect("examples/hello.pyfun must type-check and compile");
}

/// And it must actually run without a Python exception. Skips when no interpreter
/// is on PATH (like the other e2e tests).
#[test]
fn e2e_the_hello_example_runs() {
    let Some(python) = python_cmd() else {
        eprintln!("skipping hello.pyfun e2e: no python interpreter found");
        return;
    };
    let src = include_str!("../examples/hello.pyfun");
    let program = pyfun::compile(src).expect("compile hello.pyfun");
    // `run_python` asserts the process exits successfully, panicking otherwise.
    let _ = run_python(&python, &program);
}

#[test]
fn e2e_numeric_conversions() {
    run_and_check(
        "
        let a = round 3.7
        let b = floor 3.7
        let c = ceil 3.2
        let d = truncate 3.9
        let e = floor (-2.5)
        let f = Option.withDefault 0.0 (String.toFloat \"3.5\")
        let g = Option.withDefault 0.0 (String.toFloat \"nope\")
        ",
        &[
            ("a", "4"),
            ("b", "3"),
            ("c", "4"),
            ("d", "3"),
            ("e", "-3"),
            ("f", "3.5"),
            ("g", "0.0"),
        ],
    );
}

#[test]
fn e2e_string_escapes() {
    // `\r`/`\u{...}` decode correctly and re-emit as valid Python (the emitter
    // re-escapes `\r`, else a raw CR would break the literal). Output is
    // encoding-independent: char counts, not the glyphs.
    run_and_check(
        "
        let crlf = String.len \"a\\r\\nb\"
        let emoji = String.len \"hi \\u{1F600}\"
        let accent = String.len \"caf\\u{e9}\"
        ",
        &[("crlf", "4"), ("emoji", "4"), ("accent", "4")],
    );
}

#[test]
fn e2e_digit_separators_and_bases() {
    run_and_check(
        "
        let a = 1_000_000
        let b = 0xFF
        let c = 0o17
        let d = 0b1010
        let e = 0xDEAD_BEEF
        ",
        &[
            ("a", "1000000"),
            ("b", "255"),
            ("c", "15"),
            ("d", "10"),
            ("e", "3735928559"),
        ],
    );
}

#[test]
fn scientific_notation_lowers_to_float() {
    let py = pyfun::compile("let a = 1e6\nlet b = 2.5e-3\nlet g = 6.674e-11").unwrap();
    assert!(py.contains("a = 1000000.0"), "{py}");
    assert!(py.contains("b = 0.0025"), "{py}");
    assert!(py.contains("g = 6.674e-11"), "{py}");
}

#[test]
fn e2e_scientific_notation() {
    run_and_check(
        "
        let a = 1e3
        let b = 2.5e-1
        let c = 1e3 + 1.0
        ",
        &[("a", "1000.0"), ("b", "0.25"), ("c", "1001.0")],
    );
}

#[test]
fn list_completeness_ops_lower_to_helpers() {
    let py = pyfun::compile(
        "let a = List.get 0 [1, 2]\n\
         let b = List.isEmpty [1]\n\
         let c = List.contains 2 [1, 2]\n\
         let d = List.concat [1] [2]\n\
         let e = List.sort [2, 1]",
    )
    .unwrap();
    // get is total (bounds-checked, no raw IndexError) and yields Some/None.
    assert!(
        py.contains("Some(xs[i]) if 0 <= i < len(xs) else None_()"),
        "{py}"
    );
    // isEmpty / contains are fully-applied pure predicates: they inline (Lever A)
    // to `not xs` / `x in xs` instead of the `_pf_is_empty` / `_pf_list_contains`
    // helpers.
    assert!(py.contains("b = not [1]"), "isEmpty inlined: {py}");
    assert!(py.contains("c = 2 in [1, 2]"), "contains inlined: {py}");
    assert!(!py.contains("_pf_is_empty"), "{py}");
    assert!(!py.contains("_pf_list_contains"), "{py}");
    assert!(py.contains("return xs + ys"), "{py}"); // concat
    assert!(py.contains("return sorted(xs)"), "{py}"); // sort
}

#[test]
fn e2e_list_completeness_ops() {
    run_and_check(
        "
        let xs = [3, 1, 2]
        let a = Option.withDefault 0 (List.get 0 xs)
        let b = Option.withDefault 0 (List.get 9 xs)
        let c = List.contains 2 xs
        let d = List.concat xs [4]
        let e = List.sort xs
        let f = Option.withDefault 0 (List.find (fun x -> x > 1) xs)
        let g = List.isEmpty []
        ",
        &[
            ("a", "3"),
            ("b", "0"),
            ("c", "True"),
            ("d", "[3, 1, 2, 4]"),
            ("e", "[1, 2, 3]"),
            ("f", "3"),
            ("g", "True"),
        ],
    );
}

#[test]
fn modulo_lowers_to_python_percent() {
    let py = pyfun::compile("let r = 10 % 3\nlet even n = n % 2 == 0").unwrap();
    assert!(py.contains("r = 10 % 3"), "{py}");
    assert!(py.contains("return n % 2 == 0"), "{py}");
}

#[test]
fn e2e_modulo() {
    run_and_check(
        "
        let a = 10 % 3
        let b = 5.5 % 2.0
        let c = -7 % 3
        let d = (%) 17 5
        ",
        // `-7 % 3 == 2` (Python modulo takes the divisor's sign).
        &[("a", "1"), ("b", "1.5"), ("c", "2"), ("d", "2")],
    );
}

#[test]
fn non_ascii_string_is_not_double_encoded() {
    // The emitted Python must contain the real characters, not the mojibake a
    // per-byte `b as char` produced (`café` -> `cafÃ©`).
    let py = pyfun::compile("let s = \"café → 🎉\"").unwrap();
    assert!(py.contains("s = \"café → 🎉\""), "{py}");
    assert!(!py.contains("Ã"), "double-encoded output: {py}");
}

#[test]
fn e2e_non_ascii_string_length() {
    // Encoding-independent (output is a plain integer, no console-encoding
    // dependency): a correctly-lexed "café" has 4 characters; the old per-byte bug
    // gave 5+. Also covers a multi-byte f-string literal chunk.
    run_and_check(
        "
        let n = String.len \"café\"
        let m = String.len f\"→🎉 {n}\"
        ",
        &[("n", "4"), ("m", "4")],
    );
}

#[test]
fn chained_comparison_lowers_to_native_python_form() {
    // Lowering to Python's own `a < b < c` is what gives evaluate-once +
    // short-circuit for free — not a desugaring to `x < y and y < z`.
    let py = pyfun::compile("let f x = 1 < x < 10\nlet g = 1 == 1 == 1").unwrap();
    assert!(py.contains("return 1 < x < 10"), "{py}");
    assert!(py.contains("g = 1 == 1 == 1"), "{py}");
    assert!(!py.contains("and"), "should not desugar to `and`: {py}");
}

#[test]
fn e2e_chained_comparisons() {
    run_and_check(
        "
        let a = 1 < 2 < 3
        let b = 3 < 2 < 1
        let c =
            let x = 5
            1 < x < 10
        let d = 1 <= 1 < 2
        ",
        &[("a", "True"), ("b", "False"), ("c", "True"), ("d", "True")],
    );
}

#[test]
fn unary_minus_lowers_with_python_precedence() {
    let py = pyfun::compile(
        "let a = -5\n\
         let b = 2 * -3\n\
         let c = -(4 + 1)\n\
         let d = 0 - -7",
    )
    .unwrap();
    assert!(py.contains("a = -5"), "{py}");
    // Unary minus binds tighter than `*`, so no parens are needed around `-3`.
    assert!(py.contains("b = 2 * -3"), "{py}");
    // A looser `+` operand is parenthesized.
    assert!(py.contains("c = -(4 + 1)"), "{py}");
    assert!(py.contains("d = 0 - -7"), "{py}");
}

#[test]
fn e2e_unary_minus() {
    run_and_check(
        "
        let a = -5
        let b = abs (-5)
        let c = 2 * -3
        let d = -(4 + 1)
        let sign =
            match -1:
                case -1: \"neg\"
                case _: \"other\"
        ",
        &[
            ("a", "-5"),
            ("b", "5"),
            ("c", "-6"),
            ("d", "-5"),
            ("sign", "neg"),
        ],
    );
}

#[test]
fn operator_section_lowers_to_a_curried_lambda() {
    // `(*)` lowers to the binary lambda; a partial application curries via
    // `functools.partial` (arity 2 is known), like any 2-arity function.
    let py = pyfun::compile("let mul = (*)\nlet double = (*) 2").unwrap();
    assert!(py.contains("mul = lambda a, b: a * b"), "{py}");
    assert!(
        py.contains("double = functools.partial(lambda a, b: a * b, 2)"),
        "{py}"
    );
}

#[test]
fn e2e_operator_sections() {
    run_and_check(
        "
        let mul = (*)
        let double = (*) 2
        let total = List.fold (+) 0 [1, 2, 3, 4]
        let cmp = (<) 2 3
        let a = mul 3 4
        let b = double 5
        ",
        &[("a", "12"), ("b", "10"), ("total", "10"), ("cmp", "True")],
    );
}

#[test]
fn composition_lowers_to_a_single_argument_lambda() {
    // `f >> g` → `fun _pf_x -> g (f _pf_x)`; the reserved param avoids capture.
    let py = pyfun::compile("let inc x = x + 1\nlet h = inc >> inc").unwrap();
    assert!(py.contains("h = lambda _pf_x: inc(inc(_pf_x))"), "{py}");
}

#[test]
fn e2e_function_composition_both_directions() {
    run_and_check(
        "
        let inc x = x + 1
        let double x = x * 2
        let ltr = inc >> double
        let rtl = inc << double
        let a = ltr 3
        let b = rtl 3
        let piped = 5 |> inc >> double
        let mapped = List.map (double >> inc) [1, 2, 3]
        let clean = (String.strip >> String.upper) \"  hi  \"
        ",
        &[
            // `>>` = double(inc 3) = 8; `<<` = inc(double 3) = 7.
            ("a", "8"),
            ("b", "7"),
            // composition binds tighter than `|>`: (inc >> double) 5 = 12.
            ("piped", "12"),
            ("mapped", "[3, 5, 7]"),
            ("clean", "HI"),
        ],
    );
}

// ---------- raw strings (`DESIGN.md` §7.1) ----------

#[test]
fn raw_string_re_escapes_backslashes_on_emit() {
    let py = pyfun::compile("let p = r\"C:\\path\"").unwrap();
    // The raw content `C:\path` re-escapes to a valid Python literal.
    assert!(py.contains("p = \"C:\\\\path\""), "{py}");
}

#[test]
fn e2e_raw_string_backslashes_are_literal() {
    // Encoding-independent checks via `String.len`: `\n` in a raw string is two chars
    // (backslash + n), and `\"` is two literal chars that don't end the string.
    run_and_check(
        "
        let path_len = String.len r\"a\\nb\"
        let quote_len = String.len r\"a\\\"b\"
        ",
        &[("path_len", "4"), ("quote_len", "4")],
    );
}

// ---------- triple-quoted strings (`DESIGN.md` §6) ----------

#[test]
fn triple_quoted_string_emits_an_escaped_single_line_literal() {
    // A multi-line `"""..."""` lowers to an ordinary Python string with the
    // newlines escaped (`"a\nb"`) — value-identical, and it keeps the emitter's
    // one-statement-per-line model (a Python triple-quoted literal would need
    // unindented continuation lines).
    let py = pyfun::compile("let doc = \"\"\"a\nb\"\"\"").unwrap();
    assert!(py.contains("doc = \"a\\nb\""), "{py}");
    // Same for the interpolated `f"""..."""`: a real Python f-string, newline escaped.
    let py = pyfun::compile("let x = 1\nlet m = f\"\"\"a {x}\nb\"\"\"").unwrap();
    assert!(py.contains("m = f\"a {x}\\nb\""), "{py}");
    // And a raw triple keeps its backslashes (re-escaped) plus the real newline.
    let py = pyfun::compile("let p = r\"\"\"C:\\path\nnext\"\"\"").unwrap();
    assert!(py.contains("p = \"C:\\\\path\\nnext\""), "{py}");
}

#[test]
fn e2e_triple_quoted_strings() {
    // A multi-line string prints on two lines; an `f"""` interpolates across
    // lines; a raw triple counts its literal backslash + real newline.
    let Some(python) = python_cmd() else {
        eprintln!("skipping end-to-end check: no python interpreter found");
        return;
    };
    let src = "let x = 7\n\
               let doc = \"\"\"first line\nsecond \"quoted\" line\"\"\"\n\
               let msg = f\"\"\"x is {x}\nx + 1 is {x + 1}\"\"\"\n\
               let a = print doc\n\
               let b = print msg\n\
               let n = print (String.len r\"\"\"a\\nb\nc\"\"\")";
    let program = pyfun::compile(src).unwrap_or_else(|e| panic!("compile failed: {e}"));
    let stdout = run_python(&python, &program);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        vec![
            "first line",
            "second \"quoted\" line",
            "x is 7",
            "x + 1 is 8",
            "6", // r"""a\nb<newline>c""" = a, \, n, b, \n, c
        ],
        "program:\n{program}"
    );
}

#[test]
fn fstring_is_an_application_argument() {
    // An f-string juxtaposed as a call argument (`print f"..."`, no parens) is a
    // single application, not two statements — `starts_atom` must accept `FStr`.
    let py = pyfun::compile("let x = 5\nlet main = print f\"x is {x}\"").unwrap();
    assert!(py.contains("main = print(f\"x is {x}\")"), "{py}");
}

#[test]
fn e2e_interpolation_debug_holes() {
    run_and_check(
        "
        let x = 3
        let y = 4
        let a = f\"{x=}\"
        let b = f\"{x = }\"
        let c = f\"{x + y=}\"
        let d = f\"{x == y}\"
        let e = f\"{x <= y}\"
        ",
        &[
            ("a", "x=3"),
            ("b", "x = 3"),
            ("c", "x + y=7"),
            ("d", "False"),
            ("e", "True"),
        ],
    );
}

#[test]
fn e2e_interpolation_hole_with_nested_string() {
    // A hole may contain a string literal (with its own quotes/braces); on the
    // Python 3.12+ target these reuse the outer quote cleanly.
    run_and_check(
        "
        let s = \"a}b\"
        let r = f\"contains: {String.contains \"}\" s}\"
        ",
        &[("r", "contains: True")],
    );
}

#[test]
fn string_functions_lower_to_builtins_and_helpers() {
    let py = pyfun::compile(
        "let n = String.len \"hi\"\n\
         let up = String.upper \"hi\"\n\
         let parts = String.split \",\" \"a,b\"\n\
         let joined = String.join \"-\" parts\n\
         let has = String.contains \"a\" \"abc\"\n\
         let s = String.fromInt 7\n\
         let chars = String.toList \"hi\"",
    )
    .unwrap();
    // Bare-builtin routes (no helper).
    assert!(py.contains("len(\"hi\")"), "{py}");
    assert!(py.contains("str(7)"), "{py}");
    assert!(py.contains("list(\"hi\")"), "{py}");
    // Emitted helpers.
    assert!(py.contains("def _pf_str_upper(s):"), "{py}");
    assert!(py.contains("return s.upper()"), "{py}");
    assert!(py.contains("def _pf_str_split(sep, s):"), "{py}");
    assert!(py.contains("return s.split(sep)"), "{py}");
    assert!(py.contains("def _pf_str_join(sep, xs):"), "{py}");
    assert!(py.contains("return sep.join(xs)"), "{py}");
    // `String.contains` is a fully-applied pure predicate: it inlines to `sub in s`
    // (Lever A), so the `_pf_str_contains` helper is never emitted.
    assert!(
        py.contains("has = \"a\" in \"abc\""),
        "contains inlined: {py}"
    );
    assert!(!py.contains("_pf_str_contains"), "{py}");
}

#[test]
fn string_to_int_lowers_to_a_try_except_and_pulls_in_the_option_prelude() {
    let py = pyfun::compile("let r = String.toInt \"42\"").unwrap();
    assert!(py.contains("class Some:"), "{py}");
    assert!(py.contains("class None_:"), "{py}");
    assert!(py.contains("def _pf_str_to_int(s):"), "{py}");
    assert!(py.contains("try:"), "{py}");
    assert!(py.contains("return Some(int(s))"), "{py}");
    assert!(py.contains("except ValueError:"), "{py}");
    assert!(py.contains("return None_()"), "{py}");
}

#[test]
fn e2e_string_operations() {
    run_and_check(
        "
        let g = String.concat \"Hello, \" \"World\"
        let up = String.upper g
        let n = String.len g
        let parts = String.split \", \" g
        let joined = String.join \"/\" parts
        let has = String.contains \"World\" g
        let r = String.replace \"o\" \"0\" g
        ",
        &[
            ("up", "HELLO, WORLD"),
            ("n", "12"),
            ("parts", "['Hello', 'World']"),
            ("joined", "Hello/World"),
            ("has", "True"),
            ("r", "Hell0, W0rld"),
        ],
    );
}

#[test]
fn e2e_string_to_int_parses_or_yields_none() {
    run_and_check(
        "
        let ok = Option.withDefault 0 (String.toInt \"41\")
        let bad = Option.withDefault 0 (String.toInt \"nope\")
        let neg = Option.withDefault 0 (String.toInt \"-5\")
        ",
        &[("ok", "41"), ("bad", "0"), ("neg", "-5")],
    );
}

#[test]
fn unused_collection_helpers_are_not_emitted() {
    let py = pyfun::compile("let s = Set.empty").unwrap();
    assert!(!py.contains("_pf_"), "{py}");
}

#[test]
fn result_map_lowers_to_a_helper_pulling_in_the_result_prelude() {
    let py = pyfun::compile("let r = Result.map (fun x -> x) (Ok 1)").unwrap();
    assert!(py.contains("class Ok:"), "{py}");
    assert!(py.contains("def _pf_result_map(f, r):"), "{py}");
    assert!(py.contains("return Ok(f(r._0))"), "{py}");
}

#[test]
fn seq_map_filter_lower_to_pythons_lazy_builtins() {
    // Unlike `List.map` (eager, wrapped in `_pf_map`), `Seq.map`/`filter` route to
    // Python's own lazy builtins with no wrapper.
    let py =
        pyfun::compile("let r = Seq.map (fun x -> x) (Seq.filter (fun x -> true) (Seq.range 0 3))")
            .unwrap();
    assert!(py.contains("map(lambda x: x"), "{py}");
    assert!(py.contains("filter(lambda x: True"), "{py}");
    assert!(py.contains("range(0, 3)"), "{py}");
    assert!(
        !py.contains("_pf_map"),
        "Seq.map must not use the eager helper: {py}"
    );
}

#[test]
fn seq_take_lowers_to_itertools_islice() {
    let py = pyfun::compile("let r = Seq.take 2 (Seq.range 0 9)").unwrap();
    assert!(py.contains("import itertools"), "{py}");
    assert!(py.contains("def _pf_seq_take(n, xs):"), "{py}");
    assert!(py.contains("return itertools.islice(xs, n)"), "{py}");
}

#[test]
fn module_members_lower_to_mangled_top_level_names() {
    let py = pyfun::compile(
        "module Geometry =\n  let pi = 3\n  let area r = pi * r * r\nlet big = Geometry.area 10",
    )
    .unwrap();
    assert!(py.contains("Geometry_pi = 3"), "{py}");
    assert!(py.contains("def Geometry_area(r):"), "{py}");
    // A bare sibling reference (`pi` inside `area`) is rewritten to the mangled name.
    assert!(py.contains("return Geometry_pi * r * r"), "{py}");
    // Qualified access from outside lowers to the same mangled name.
    assert!(py.contains("big = Geometry_area(10)"), "{py}");
}

#[test]
fn partial_application_of_a_module_member() {
    let py = pyfun::compile("module M =\n  let add a b = a + b\nlet inc = M.add 1").unwrap();
    assert!(py.contains("inc = functools.partial(M_add, 1)"), "{py}");
}

#[test]
fn result_to_option_pulls_in_both_preludes() {
    let py = pyfun::compile("let o = Result.toOption (Ok 1)").unwrap();
    assert!(py.contains("class Ok:"), "Result prelude: {py}");
    assert!(py.contains("class Some:"), "Option prelude: {py}");
    assert!(py.contains("def _pf_result_to_option(r):"), "{py}");
    assert!(py.contains("return Some(r._0)"), "{py}");
    assert!(py.contains("return None_()"), "{py}");
}

#[test]
fn if_in_value_position_is_a_conditional_expression() {
    let py = pyfun::compile("let r = if true then 1 else 2").unwrap();
    assert!(py.contains("r = 1 if True else 2"), "{py}");
}

#[test]
fn exhaustive_match_without_wildcard_keeps_a_runtime_guard() {
    // Even a statically exhaustive ADT match emits a defensive `case _: raise`.
    let py = pyfun::compile("type Color = Red | Green | Blue\nlet f c = match c: case Red: 1 case Green: 2 case Blue: 3").unwrap();
    assert!(py.contains("case _:"), "{py}");
    assert!(
        py.contains("raise RuntimeError(\"non-exhaustive match\")"),
        "{py}"
    );
}

#[test]
fn adt_lowers_to_frozen_dataclasses() {
    // A user-defined ADT (Option/Some/None are now built-in, so use a fresh type).
    // Each variant is a frozen dataclass — the decorator generates __init__/__eq__/
    // __hash__/__match_args__ from the field annotations, and `frozen` makes the value
    // immutable (matching Pyfun). The import is emitted once.
    let py = pyfun::compile("type Opt a = Empty | Has a\nlet x = Has 1").unwrap();
    assert!(py.contains("from dataclasses import dataclass"), "{py}");
    assert!(py.contains("@dataclass(frozen=True"), "{py}");
    assert!(py.contains("class Has:"), "{py}");
    assert!(py.contains("_0: object"), "{py}"); // the field the dataclass derives from
    assert!(py.contains("class Empty:"), "{py}");
    assert!(py.contains("x = Has(1)"), "{py}");
}

#[test]
fn adt_match_args_work_positionally() {
    // The dataclass-generated __match_args__ lets `case Has(v)` bind positionally.
    run_and_check(
        "type Opt a = Empty | Has a\n\
         let unwrap o =\n  match o:\n    case Has v: v\n    case Empty: 0\n\
         let r = unwrap (Has 7)",
        &[("r", "7")],
    );
}

#[test]
fn adt_classes_get_a_repr() {
    let py = pyfun::compile("type Opt a = Empty | Has a\nlet x = Has 1").unwrap();
    assert!(py.contains("def __repr__(self):"), "{py}");
    // Nullary uses the bare class name; a field uses `!r`.
    assert!(py.contains("return \"Empty\""), "{py}");
    assert!(py.contains("return f\"Has({self._0!r})\""), "{py}");
}

#[test]
fn adt_classes_get_structural_eq() {
    // `==` compares by constructor + fields (the dataclass __eq__), not identity.
    run_and_check(
        "type Opt a = Empty | Has a\nlet a = Has 1 == Has 1\nlet b = Has 1 == Has 2",
        &[("a", "True"), ("b", "False")],
    );
}

#[test]
fn adt_classes_get_structural_hash() {
    // Structural `__hash__` (from the frozen dataclass) so ADTs are `Set` elements /
    // `Map` keys — defining `__eq__` alone would make them unhashable in Python.
    run_and_check(
        "type Color = Red | Green | Blue\n\
         let s = Set.ofList [Red, Red, Green]\n\
         let n = Set.len s\n\
         let hasRed = Set.contains Red s\n\
         let hasBlue = Set.contains Blue s",
        &[("n", "2"), ("hasRed", "True"), ("hasBlue", "False")],
    );
}

#[test]
fn adt_classes_get_derived_ordering() {
    // A *compared* user variant class gets `<`/`<=`/`>`/`>=` keyed on
    // `(variant_index, fields…)`; the index sorts by declaration order. (Ordering is
    // emitted on demand — the `Red < Green` here is what makes `Color` orderable.)
    let py = pyfun::compile("type Color = Red | Green | Blue\nlet x = Red < Green").unwrap();
    assert!(py.contains("def _pf_order_key(self):"), "{py}");
    assert!(py.contains("def __lt__(self, other):"), "{py}");
    assert!(
        py.contains("return self._pf_order_key() < other._pf_order_key()"),
        "{py}"
    );
    // Nullary variants key on just the index (a 1-tuple); indices are 0, 1, 2.
    assert!(py.contains("return (0,)"), "Red index 0: {py}");
    assert!(py.contains("return (1,)"), "Green index 1: {py}");
    assert!(py.contains("return (2,)"), "Blue index 2: {py}");
}

#[test]
fn ordering_is_emitted_only_when_a_type_is_compared() {
    // On-demand ordering (`DESIGN.md` §7.1): a sum type that is only matched/constructed,
    // never compared, sheds `_pf_order_key`/`__lt__`/`@total_ordering` entirely.
    let py = pyfun::compile("type Color = Red | Green | Blue\nlet x = Red").unwrap();
    assert!(py.contains("class Red:"), "{py}");
    assert!(
        !py.contains("_pf_order_key"),
        "no ordering when uncompared: {py}"
    );
    assert!(
        !py.contains("total_ordering"),
        "no import when uncompared: {py}"
    );
}

#[test]
fn payload_variant_order_key_includes_fields() {
    let py = pyfun::compile(
        "type Shape = Circle float | Rect float float\nlet c = Circle 1.0 < Rect 0.0 0.0",
    )
    .unwrap();
    assert!(py.contains("return (0, self._0)"), "Circle: {py}");
    assert!(py.contains("return (1, self._0, self._1)"), "Rect: {py}");
}

#[test]
fn builtin_option_and_result_get_ordering_but_exception_does_not() {
    // `Some`/`None_`/`Ok`/`Error` derive ordering like a user sum type *when compared*;
    // the reserved `Exception` record (the `try` payload) never does.
    let py = pyfun::compile("let x = Some 1 < Some 2").unwrap();
    assert!(py.contains("class Some:"), "{py}");
    assert!(py.contains("_pf_order_key"), "Option gets ordering: {py}");
    let py = pyfun::compile("let x = try (Some 1)").unwrap();
    assert!(py.contains("class _Exception:"), "{py}");
    // The Exception class has no ordering key.
    assert!(
        !py.contains("(0, self.errorKind"),
        "Exception gets no ordering: {py}"
    );
}

#[test]
fn e2e_sort_a_sum_type_by_variant_order() {
    run_and_check(
        "
        type Color = Red | Green | Blue
        let sorted = List.sort [Blue, Red, Green]
        let a = Red < Green
        let b = Green < Blue
        ",
        &[
            ("sorted", "[Red, Green, Blue]"),
            ("a", "True"),
            ("b", "True"),
        ],
    );
}

#[test]
fn e2e_sum_orders_by_variant_then_field() {
    run_and_check(
        "
        type Shape = Circle float | Rect float float
        let a = Circle 1.0 < Circle 2.0
        let b = Circle 9.0 < Rect 0.0 0.0
        let c = Rect 1.0 2.0 < Rect 1.0 3.0
        ",
        &[("a", "True"), ("b", "True"), ("c", "True")],
    );
}

#[test]
fn e2e_sort_records_and_tuples() {
    run_and_check(
        "
        type Point = { x: int, y: int }
        let pts = List.sort [Point { x = 2, y = 0 }, Point { x = 1, y = 9 }, Point { x = 1, y = 3 }]
        let tups = List.sort [(1, 3), (1, 2), (0, 9)]
        ",
        // Records sort field-by-field; tuples lexicographically. A record's repr names
        // its fields (the dataclass default), `Point(x=1, y=3)`.
        &[
            ("pts", "[Point(x=1, y=3), Point(x=1, y=9), Point(x=2, y=0)]"),
            ("tups", "[(0, 9), (1, 2), (1, 3)]"),
        ],
    );
}

#[test]
fn e2e_sort_a_recursive_type() {
    run_and_check(
        "
        type Tree = Leaf int | Node Tree Tree
        let sorted = List.sort [Node (Leaf 2) (Leaf 3), Leaf 5, Leaf 1]
        ",
        // Leaf (variant 0) < Node (variant 1); Leaf 1 < Leaf 5 by field.
        &[("sorted", "[Leaf(1), Leaf(5), Node(Leaf(2), Leaf(3))]")],
    );
}

#[test]
fn record_lowers_to_frozen_dataclass() {
    let py =
        pyfun::compile("type Point = { x: int, y: int }\nlet p = Point { y = 4, x = 3 }").unwrap();
    // A record is a frozen dataclass with Python-typed field annotations. It keeps the
    // dataclass-generated repr (named fields), so no custom `__repr__` is emitted.
    assert!(py.contains("@dataclass(frozen=True"), "{py}");
    assert!(py.contains("class Point:"), "{py}");
    assert!(py.contains("x: int"), "{py}");
    assert!(py.contains("y: int"), "{py}");
    assert!(
        !py.contains("def __repr__"),
        "records use the dataclass repr: {py}"
    );
    // The literal is reordered to the declared field order for a positional call.
    assert!(py.contains("p = Point(3, 4)"), "{py}");
    // Uncompared here, so no `order=True` (ordering is on demand, `DESIGN.md` §7.1).
    assert!(
        !py.contains("order=True"),
        "no ordering when uncompared: {py}"
    );
}

#[test]
fn a_compared_record_gets_order_true() {
    // Comparing a record makes it orderable via `@dataclass(order=True)` (field-tuple
    // compare) — no cross-variant key needed, unlike a sum variant.
    let py = pyfun::compile(
        "type Point = { x: int, y: int }\nlet a = Point { x = 1, y = 2 } < Point { x = 1, y = 3 }",
    )
    .unwrap();
    assert!(py.contains("@dataclass(frozen=True, order=True"), "{py}");
}

#[test]
fn dataclass_fields_get_python_type_annotations() {
    // Concrete builtins map to their Python type; a type variable / user type / Option
    // maps to `object` (mapping a user type name would risk a forward reference).
    let py = pyfun::compile(
        "type Opt a = None2 | Has a\n\
         type Rec = { count: int, ratio: float, label: string, tags: List string, maybe: Opt int }\n\
         let r = Rec { count = 1, ratio = 2.0, label = \"x\", tags = [], maybe = None2 }",
    )
    .unwrap();
    assert!(py.contains("count: int"), "{py}");
    assert!(py.contains("ratio: float"), "{py}");
    assert!(py.contains("label: str"), "{py}");
    assert!(py.contains("tags: list"), "{py}");
    assert!(
        py.contains("maybe: object"),
        "user-typed field is object: {py}"
    );
    // A generic ADT payload (`Has a`) is also `object`.
    assert!(py.contains("_0: object"), "{py}");
}

#[test]
fn record_update_copies_through_a_temp() {
    let py = pyfun::compile(
        "type Point = { x: int, y: int }\nlet p = Point { x = 1, y = 2 }\nlet q = { p with x = 9 }",
    )
    .unwrap();
    // `p` is bound to a temp so it is evaluated once; the unchanged field is read
    // from it, the changed one is the new value.
    assert!(py.contains("q = Point(9, _pf_t0.y)"), "{py}");
}

#[test]
fn record_field_access_lowers_to_attribute() {
    let py =
        pyfun::compile("type Point = { x: int }\nlet p = Point { x = 1 }\nlet s = p.x").unwrap();
    assert!(py.contains("s = p.x"), "{py}");
}

#[test]
fn record_pattern_lowers_to_keyword_class_pattern() {
    let py = pyfun::compile(
        "type Point = { x: int, y: int }\n\
         let f p = match p: case Point { x = 0, y }: y case Point { x }: x",
    )
    .unwrap();
    // Keyword class patterns name a subset of fields, in source order.
    assert!(py.contains("case Point(x=0, y=y):"), "{py}");
    assert!(py.contains("case Point(x=x):"), "{py}");
}

#[test]
fn tuple_literal_lowers_to_python_tuple() {
    let py = pyfun::compile("let pair = (1, 2)\nlet triple = (1, true, 3)").unwrap();
    assert!(py.contains("pair = (1, 2)"), "{py}");
    assert!(py.contains("triple = (1, True, 3)"), "{py}");
}

#[test]
fn tuple_pattern_lowers_to_sequence_pattern() {
    let py = pyfun::compile("let swap p = match p: case (a, b): (b, a)").unwrap();
    assert!(py.contains("case (a, b):"), "{py}");
    assert!(py.contains("return (b, a)"), "{py}");
}

#[test]
fn e2e_tuple_construct_and_destructure() {
    run_and_check(
        "
        let swap p =
          match p:
            case (a, b): (b, a)
        let fst t =
          match t:
            case (a, _): a
        let pair = (10, 20)
        let s = swap pair
        let first = fst pair
        ",
        &[("s", "(20, 10)"), ("first", "10")],
    );
}

#[test]
fn e2e_nested_tuple_pattern() {
    run_and_check(
        "
        let f t =
          match t:
            case ((a, b), c): a + b + c
        let r = f ((1, 2), 3)
        ",
        &[("r", "6")],
    );
}

#[test]
fn list_pattern_lowers_to_a_bracket_sequence_pattern() {
    // List patterns emit bracket sequence patterns (`case [..]`), distinct from a
    // tuple's paren `case (..)`; the star becomes a Python `*rest` capture.
    let py =
        pyfun::compile("let f xs =\n  match xs:\n    case []: 0\n    case [x, *rest]: x").unwrap();
    assert!(py.contains("case []:"), "{py}");
    assert!(py.contains("case [x, *rest]:"), "{py}");
}

#[test]
fn wildcard_rest_lowers_to_star_underscore() {
    let py =
        pyfun::compile("let f xs =\n  match xs:\n    case [x, *_]: x\n    case []: 0").unwrap();
    assert!(py.contains("case [x, *_]:"), "{py}");
}

#[test]
fn suffix_star_lowers_to_a_python_mid_star_sequence_pattern() {
    // Python allows one star anywhere in a sequence pattern, so `[*init, last]`
    // and `[a, *mid, z]` lower 1:1.
    let py = pyfun::compile(
        "let f xs =\n\
         \x20 match xs:\n\
         \x20 \x20 case []: 0\n\
         \x20 \x20 case [x]: x\n\
         \x20 \x20 case [a, *mid, z]: a + z",
    )
    .unwrap();
    assert!(py.contains("case [a, *mid, z]:"), "{py}");
    let py =
        pyfun::compile("let f xs =\n  match xs:\n    case [*init, last]: last\n    case []: 0")
            .unwrap();
    assert!(py.contains("case [*init, last]:"), "{py}");
}

#[test]
fn e2e_suffix_and_mid_star_patterns() {
    run_and_check(
        "
        let lastOr fallback xs =
          match xs:
            case [*_, last]: last
            case []: fallback
        let bounds xs =
          match xs:
            case []: 0
            case [x]: x
            case [first, *mid, last]: first + last + List.len mid
        let a = lastOr 0 [5, 6, 7]
        let b = lastOr 9 []
        let c = bounds [1, 2, 3, 4]
        let d = bounds [7]
        ",
        &[("a", "7"), ("b", "9"), ("c", "7"), ("d", "7")],
    );
}

#[test]
fn e2e_list_sum_with_sequence_patterns() {
    run_and_check(
        "
        let sum xs =
          match xs:
            case []: 0
            case [x, *rest]: x + sum rest
        let total = sum [1, 2, 3, 4]
        let empty = sum []
        ",
        &[("total", "10"), ("empty", "0")],
    );
}

#[test]
fn e2e_list_fixed_length_and_nested_patterns() {
    run_and_check(
        "
        let describe xs =
          match xs:
            case []: 0
            case [x]: 1
            case [x, y]: 2
            case [x, y, *rest]: 3
        let head xs =
          match xs:
            case [Some x, *rest]: x
            case _: 0
        let a = describe [10, 20, 30]
        let b = describe [7]
        let c = describe []
        let d = head [Some 42, None]
        ",
        &[("a", "3"), ("b", "1"), ("c", "0"), ("d", "42")],
    );
}

#[test]
fn e2e_record_pattern_match() {
    run_and_check(
        "
        type Point = { x: int, y: int }
        let classify p =
          match p:
            case Point { x = 0, y = 0 }: 1
            case Point { x = 0 }: 2
            case Point { x, y }: x + y
        let a = classify (Point { x = 0, y = 0 })
        let b = classify (Point { x = 0, y = 9 })
        let c = classify (Point { x = 3, y = 4 })
        ",
        &[("a", "1"), ("b", "2"), ("c", "7")],
    );
}

#[test]
fn e2e_nested_record_and_constructor_pattern() {
    // A constructor sub-pattern inside a record pattern binds through both levels.
    run_and_check(
        "
        type Box = { item: Option int, tag: bool }
        let f b =
          match b:
            case Box { item = Some n, tag = true }: n
            case Box { item = Some n }: n + 100
            case _: 0
        let a = f (Box { item = Some 5, tag = true })
        let b = f (Box { item = Some 5, tag = false })
        let c = f (Box { item = None, tag = true })
        ",
        &[("a", "5"), ("b", "105"), ("c", "0")],
    );
}

#[test]
fn e2e_deep_exhaustive_match_without_wildcard() {
    // Deep exhaustiveness accepts this (every nested case covered), and it runs.
    run_and_check(
        "
        let f o =
          match o:
            case Some true: 1
            case Some false: 2
            case None: 3
        let a = f (Some true)
        let b = f (Some false)
        let c = f None
        ",
        &[("a", "1"), ("b", "2"), ("c", "3")],
    );
}

#[test]
fn block_body_lowers_to_statement_sequence() {
    let py = pyfun::compile(
        "let sum3 a b c =\n    let mut acc = 0\n    acc <- acc + a\n    acc <- acc + b\n    acc",
    )
    .unwrap();
    assert!(py.contains("def sum3(a, b, c):"), "{py}");
    assert!(py.contains("    acc = 0"), "{py}");
    assert!(py.contains("    acc = acc + a"), "{py}");
    assert!(py.contains("    return acc"), "{py}");
}

#[test]
fn top_level_assignment_lowers_to_plain_assign() {
    let py = pyfun::compile("let mut x = 0\nx <- x + 1").unwrap();
    assert!(py.contains("x = 0"), "{py}");
    assert!(py.contains("x = x + 1"), "{py}");
    // No bare `None` line from the unit-valued assignment statement.
    assert!(!py.contains("\nNone"), "{py}");
}

#[test]
fn closure_reassigning_a_module_mut_emits_global() {
    // The enclosing block inlines at module level, so the captured `n` is global.
    let py = pyfun::compile(
        "let counter =\n  let mut n = 0\n  let bump x =\n    n <- n + x\n    n\n  bump 5",
    )
    .unwrap();
    assert!(py.contains("def bump(x):"), "{py}");
    assert!(py.contains("global n"), "{py}");
    assert!(!py.contains("nonlocal"), "{py}");
}

#[test]
fn closure_reassigning_an_enclosing_fn_mut_emits_nonlocal() {
    let py = pyfun::compile(
        "let make base =\n  let mut n = base\n  let bump x =\n    n <- n + x\n    n\n  bump 5",
    )
    .unwrap();
    assert!(py.contains("def make(base):"), "{py}");
    assert!(py.contains("nonlocal n"), "{py}");
    assert!(!py.contains("global"), "{py}");
}

#[test]
fn local_only_mut_needs_no_capture_declaration() {
    let py =
        pyfun::compile("let scaled a b =\n  let mut acc = a\n  acc <- acc + b\n  acc").unwrap();
    assert!(!py.contains("nonlocal"), "{py}");
    assert!(!py.contains("global"), "{py}");
}

#[test]
fn e2e_closure_reassigns_captured_mut() {
    // `global` path: the accumulator persists across calls (5 then +3 = 8).
    run_and_check(
        "
        let counter =
          let mut n = 0
          let bump x =
            n <- n + x
            n
          let a = bump 5
          bump 3
        ",
        &[("counter", "8")],
    );
}

#[test]
fn e2e_nonlocal_closure_in_a_function() {
    // `nonlocal` path: a fresh accumulator per `make` call.
    run_and_check(
        "
        let make base =
          let mut n = base
          let bump x =
            n <- n + x
            n
          let a = bump 5
          bump 3
        let r = make 100
        ",
        &[("r", "108")],
    );
}

#[test]
fn nested_local_let_lowers_to_nested_assignments() {
    let py = pyfun::compile(
        "let f x =\n    let y =\n        let mut t = x\n        t <- t + 1\n        t\n    y",
    )
    .unwrap();
    assert!(py.contains("def f(x):"), "{py}");
    assert!(py.contains("t = x"), "{py}");
    assert!(py.contains("t = t + 1"), "{py}");
    assert!(py.contains("y = t"), "{py}");
    assert!(py.contains("return y"), "{py}");
}

#[test]
fn pure_modifier_is_erased_at_lowering() {
    // Effects (and the `pure` assertion) leave no runtime residue.
    let py = pyfun::compile("let pure add a b = a + b\nlet r = add 1 2").unwrap();
    assert!(py.contains("def add(a, b):"), "{py}");
    assert!(!py.contains("pure"), "{py}");
    assert!(!py.contains("io"), "{py}");
}

#[test]
fn comparison_operators_lower_to_python() {
    let py =
        pyfun::compile("let a = 1 < 2\nlet b = 1 == 2\nlet c = 1 != 2\nlet d = 1 >= 2").unwrap();
    assert!(py.contains("a = 1 < 2"), "{py}");
    assert!(py.contains("b = 1 == 2"), "{py}");
    assert!(py.contains("c = 1 != 2"), "{py}");
    assert!(py.contains("d = 1 >= 2"), "{py}");
}

#[test]
fn boolean_operators_lower_to_python_keywords_with_precedence() {
    // `and`/`or`/`not` lower to the same Python keywords. Precedence mirrors
    // Python, so no redundant parentheses, but looser operands under `not` get them.
    assert!(
        pyfun::compile("let r = true and false")
            .unwrap()
            .contains("r = True and False")
    );
    assert!(
        pyfun::compile("let r = true or false")
            .unwrap()
            .contains("r = True or False")
    );
    assert!(
        pyfun::compile("let r = not true")
            .unwrap()
            .contains("r = not True")
    );
    // `not (1 == 2)` needs no parens (not is looser than ==, as in Python).
    assert!(
        pyfun::compile("let r = not 1 == 2")
            .unwrap()
            .contains("r = not 1 == 2")
    );
    // `(not true) == false` does need them.
    assert!(
        pyfun::compile("let r = (not true) == false")
            .unwrap()
            .contains("r = (not True) == False")
    );
}

#[test]
fn prelude_partial_application_uses_partial() {
    // A partially applied builtin must close over its arg, not call `max(0)`.
    let py = pyfun::compile("let clamp0 = max 0").unwrap();
    assert!(py.contains("clamp0 = functools.partial(max, 0)"), "{py}");
}

#[test]
fn unknown_constructor_is_rejected() {
    let err = pyfun::compile("let f o = match o: case Nope v: v").unwrap_err();
    assert!(err.to_string().contains("unknown constructor"), "{err}");
}

// ---------- end-to-end execution ----------

#[test]
fn recursive_call_lowers_to_a_direct_call() {
    let py = pyfun::compile("let fact n =\n  if n == 0 then 1\n  else n * fact (n - 1)").unwrap();
    assert!(py.contains("def fact(n):"), "{py}");
    // The self-call is a full application (arity known), not a functools.partial.
    assert!(py.contains("fact(n - 1)"), "{py}");
}

#[test]
fn e2e_recursive_functions() {
    run_and_check(
        "
        let fact n =
          if n == 0 then 1
          else n * fact (n - 1)
        let fib n =
          if n < 2 then n
          else fib (n - 1) + fib (n - 2)
        let a = fact 6
        let b = fib 10
        ",
        &[("a", "720"), ("b", "55")],
    );
}

#[test]
fn e2e_currying_full_partial_and_over_application() {
    run_and_check(
        "
        let add a b = a + b
        let inc = add 1
        let twice f x = f (f x)
        let r1 = add 1 2
        let r2 = inc 41
        let r3 = twice inc 5
        ",
        &[("r1", "3"), ("r2", "42"), ("r3", "7")],
    );
}

#[test]
fn e2e_pipe_and_composition() {
    run_and_check(
        "
        let add a b = a + b
        let inc = add 1
        let compose = fun f g x -> f (g x)
        let r = 4 |> inc |> inc
        let r2 = (compose inc inc) 10
        ",
        &[("r", "6"), ("r2", "12")],
    );
}

#[test]
fn backward_pipe_lowers_to_application() {
    // `f <| x` is `f(x)`; `f <| g <| x` is right-associative `f(g(x))`.
    let py =
        pyfun::compile("let inc n = n + 1\nlet a = inc <| 5\nlet b = inc <| inc <| 5").unwrap();
    assert!(py.contains("a = inc(5)"), "{py}");
    assert!(py.contains("b = inc(inc(5))"), "{py}");
}

#[test]
fn e2e_backward_pipe() {
    run_and_check(
        "
        let inc n = n + 1
        let twice n = n * 2
        let a = inc <| 5
        let b = inc <| twice <| 5
        let c = twice <| inc 5
        ",
        // b: inc(twice(5))=11; c: twice(inc(5))=12.
        &[("a", "6"), ("b", "11"), ("c", "12")],
    );
}

#[test]
fn e2e_elif_chain_selects_the_right_branch() {
    // `elif` is sugar for `else if`; the chain compiles to nested conditionals and
    // picks the first matching branch (here via nested ternaries, all-expression).
    run_and_check(
        "
        let grade n =
          if n >= 90 then \"A\"
          elif n >= 80 then \"B\"
          elif n >= 70 then \"C\"
          else \"F\"
        let a = grade 95
        let b = grade 85
        let c = grade 72
        let f = grade 50
        ",
        &[("a", "A"), ("b", "B"), ("c", "C"), ("f", "F")],
    );
}

#[test]
fn e2e_if_and_match() {
    run_and_check(
        "
        let classify n =
          match n:
            case 0: \"zero\"
            case 1: \"one\"
            case _: \"many\"
        let r1 = classify 0
        let r2 = classify 7
        let r3 = if true then 10 else 20
        ",
        &[("r1", "zero"), ("r2", "many"), ("r3", "10")],
    );
}

#[test]
fn e2e_blocks_in_match_arms_and_if_branches() {
    // Multi-statement blocks (with local `let` and `<-`) in arm and branch
    // positions, lowered to flat Python statement sequences.
    run_and_check(
        "
        let classify n =
          match n:
            case 0:
              let base = 100
              base
            case _:
              let mut acc = n
              acc <- acc * 2
              acc
        let absdiff a b =
          if a > b then
              let d = a - b
              d
          else
              let d = b - a
              d
        let r1 = classify 0
        let r2 = classify 5
        let r3 = absdiff 3 10
        ",
        &[("r1", "100"), ("r2", "10"), ("r3", "7")],
    );
}

#[test]
fn e2e_block_in_lambda_body() {
    run_and_check(
        "
        let doubler = fun x ->
          let y = x + x
          y
        let r = doubler 21
        ",
        &[("r", "42")],
    );
}

#[test]
fn e2e_division_operators_match_python() {
    // `/` is true division (float), `//` floors (int) — like Python 3.
    run_and_check("let q = 7 / 2", &[("q", "3.5")]);
    run_and_check("let d = 7 // 2", &[("d", "3")]);
}

#[test]
fn division_operators_lower_to_matching_python_operators() {
    assert!(
        pyfun::compile("let q = 7 / 2")
            .unwrap()
            .contains("q = 7 / 2")
    );
    assert!(
        pyfun::compile("let d = 7 // 2")
            .unwrap()
            .contains("d = 7 // 2")
    );
}

#[test]
fn e2e_match_in_value_position_is_hoisted() {
    // The match must be evaluated into a temp, then added to 5.
    run_and_check(
        "let r = (match 1: case 1: 10 case _: 20) + 5",
        &[("r", "15")],
    );
}

#[test]
fn e2e_adt_construction_and_match() {
    run_and_check(
        "
        type Color = Red | Green | Blue
        let unwrap o = match o: case Some v: v case None: 0
        let r1 = unwrap (Some 5)
        let r2 = unwrap None
        let rank c = match c: case Red: 1 case Green: 2 case Blue: 3
        let r3 = rank Green
        ",
        &[("r1", "5"), ("r2", "0"), ("r3", "2")],
    );
}

#[test]
fn e2e_records_construct_access_and_update() {
    run_and_check(
        "
        type Point = { x: int, y: int }
        let p = Point { x = 3, y = 4 }
        let moved = { p with x = 10 }
        let sx = p.x
        let sy = moved.y
        let mx = moved.x
        let sumxy r = r.x + r.y
        let total = sumxy p
        ",
        &[("sx", "3"), ("sy", "4"), ("mx", "10"), ("total", "7")],
    );
}

#[test]
fn e2e_nested_record_update() {
    // `{ o with inner.a = v }` rebuilds only the touched path; sibling fields
    // (`inner.b`, `tag`) are preserved from the (once-evaluated) base.
    run_and_check(
        "
        type Inner = { a: int, b: int }
        type Outer = { inner: Inner, tag: string }
        let o = Outer { inner = Inner { a = 1, b = 2 }, tag = \"x\" }
        let o2 = { o with inner.a = 99 }
        let both = { o with inner.a = 10, tag = \"y\" }
        let na = o2.inner.a
        let nb = o2.inner.b
        let nt = o2.tag
        let ba = both.inner.a
        let bt = both.tag
        ",
        &[
            ("na", "99"),
            ("nb", "2"),
            ("nt", "x"),
            ("ba", "10"),
            ("bt", "y"),
        ],
    );
}

#[test]
fn e2e_polymorphic_record_field() {
    run_and_check(
        "
        type Box a = { item: a }
        let mk v = Box { item = v }
        let i = (mk 42).item
        let s = (mk \"hi\").item
        ",
        &[("i", "42"), ("s", "hi")],
    );
}

#[test]
fn e2e_blocks_and_mutation() {
    run_and_check(
        "
        let sum3 a b c =
            let mut acc = 0
            acc <- acc + a
            acc <- acc + b
            acc <- acc + c
            acc
        let nested x =
            let y =
                let mut t = x
                t <- t * 2
                t
            y + 1
        let r1 = sum3 1 2 3
        let r2 = nested 10
        ",
        &[("r1", "6"), ("r2", "21")],
    );
}

#[test]
fn e2e_top_level_mutation() {
    run_and_check(
        "
        let mut counter = 0
        counter <- counter + 1
        counter <- counter + 5
        ",
        &[("counter", "6")],
    );
}

#[test]
fn e2e_recursive_adt() {
    // A cons-list: length via recursion-free folding isn't available, but nested
    // construction and matching exercise recursive types end to end.
    run_and_check(
        // `List` is now a built-in collection type, so this cons-list ADT uses a
        // distinct name (`Lst`).
        "
        type Lst a = Nil | Cons a (Lst a)
        let head d xs = match xs: case Nil: d case Cons h t: h
        let r = head 0 (Cons 7 (Cons 8 Nil))
        ",
        &[("r", "7")],
    );
}

#[test]
fn units_are_erased_in_emitted_python() {
    let py = pyfun::compile("measure m\nmeasure s\nlet speed = 100<m> / 10<s>").unwrap();
    assert!(!py.contains('<'), "units should be erased: {py}");
    assert!(py.contains("speed = 100 / 10"), "{py}");
}

#[test]
fn e2e_units_compute_after_erasure() {
    run_and_check(
        "
        measure m
        measure s
        let dist = 100<m>
        let time = 10<s>
        let speed = dist / time
        ",
        // `/` is true division, so the unit-bearing result is a float.
        &[("speed", "10.0")],
    );
}

#[test]
fn e2e_derived_measure_aliases_erase() {
    // Aliases are pure compile-time machinery; like base units they vanish at
    // runtime, leaving plain Python numbers.
    run_and_check(
        "
        measure kg
        measure m
        measure s
        measure N = kg m / s^2
        measure Pa = N / m^2
        let force = 10<N>
        let area = 2<m^2>
        let pressure = force / area
        ",
        &[("force", "10"), ("pressure", "5.0")],
    );
}

#[test]
fn e2e_result_ce_binds_and_short_circuits() {
    // Extract the result via a match so the assertions compare plain ints.
    run_and_check(
        "
        let safe ok v = result { let! x = if ok then Ok v else Error 9  return (x + 1) }
        let unwrap r = match r: case Ok n: n case Error e: e
        let r1 = unwrap (safe true 10)
        let r2 = unwrap (safe false 10)
        ",
        &[("r1", "11"), ("r2", "9")],
    );
}

#[test]
fn e2e_seq_ce_produces_a_generator() {
    let Some(python) = python_cmd() else { return };
    let mut program =
        pyfun::compile("let xs = seq { yield 1  yield! (seq { yield 2  yield 3 }) }").unwrap();
    program.push_str("\nprint(list(xs))\n");
    assert_eq!(run_python(&python, &program).trim(), "[1, 2, 3]");
}

#[test]
fn e2e_async_ce_produces_a_coroutine() {
    let Some(python) = python_cmd() else { return };
    let mut program =
        pyfun::compile("let twice = async { let! x = async { return 21 }  return (x + x) }")
            .unwrap();
    program.push_str("\nimport asyncio\nprint(asyncio.run(twice))\n");
    assert_eq!(run_python(&python, &program).trim(), "42");
}

#[test]
fn e2e_format_module_formats_numbers_and_strings() {
    // The `Format` members run and produce the expected strings. Uses an ASCII `$`
    // (not `£`) so the assertion doesn't depend on the console's output encoding.
    let Some(python) = python_cmd() else { return };
    let src = "let a = Format.fixed 2 3.14159\n\
               let b = Format.thousands 2 1234567.5\n\
               let c = Format.percent 1 0.256\n\
               let d = Format.currency \"$\" 2 1234.5\n\
               let e = Format.grouped 1234567\n\
               let f = Format.padLeft 6 \"0\" \"42\"\n\
               let g = Format.padRight 6 \".\" \"42\"";
    let mut program = pyfun::compile(src).unwrap();
    program.push_str("\nfor s in [a, b, c, d, e, f, g]:\n    print(s)\n");
    // Normalize CRLF: Python prints `\r\n` line endings on Windows.
    let out = run_python(&python, &program).replace("\r\n", "\n");
    assert_eq!(
        out.trim(),
        "3.14\n1,234,567.50\n25.6%\n$1,234.50\n1,234,567\n000042\n42...."
    );
}

#[test]
fn e2e_user_monad_ce_binds_and_short_circuits() {
    // A user-defined `Maybe` builder desugars to bind/return_ calls and runs.
    run_and_check(
        "
        module Maybe =
          let bind m f = match m: case Some x: f x case None: None
          let return_ x = Some x
        let safe a b =
          Maybe {
            let! x = a
            let! y = b
            return x + y
          }
        let unwrap m = match m: case Some n: n case None: 0
        let r1 = unwrap (safe (Some 3) (Some 4))
        let r2 = unwrap (safe (Some 3) None)
        ",
        &[("r1", "7"), ("r2", "0")],
    );
}

#[test]
fn e2e_user_yield_ce_combines_via_delay() {
    // A yield builder exercising yield_/combine/delay (here, summation).
    run_and_check(
        "
        module Sum =
          let yield_ x = x
          let combine a b = a + b
          let delay f = f 0
        let total =
          Sum {
            yield 1
            yield 2
            yield 3
            yield 4
          }
        ",
        &[("total", "10")],
    );
}

#[test]
fn user_ce_lowers_to_plain_calls() {
    let py = pyfun::compile(
        "
        module M =
          let bind m f = f m
          let return_ x = x
        let r = M { let! x = 3  return x }
        ",
    )
    .unwrap();
    assert!(py.contains("r = M_bind(3, lambda x: M_return_(x))"), "{py}");
}

#[test]
fn e2e_prelude_print_and_numerics() {
    // Bare `print` statements (separated by the offside rule) and the numeric
    // builtins run end to end and produce observable output.
    let Some(python) = python_cmd() else {
        eprintln!("skipping end-to-end check: no python interpreter found");
        return;
    };
    let program = pyfun::compile(
        "let a = 3\n\
         let b = 10\n\
         print (min a b)\n\
         print (max a b)\n\
         print (abs (a - b))\n\
         print (Some 7)",
    )
    .unwrap();
    let stdout = run_python(&python, &program);
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        ["3", "10", "7", "Some(7)"]
    );
}

#[test]
fn e2e_comparison_and_structural_equality() {
    let Some(python) = python_cmd() else {
        eprintln!("skipping end-to-end check: no python interpreter found");
        return;
    };
    let program = pyfun::compile(
        "print (1 < 2)\n\
         print (\"a\" < \"b\")\n\
         print (3 == 3)\n\
         print (Some 1 == Some 1)\n\
         print (Some 1 == Some 2)",
    )
    .unwrap();
    let stdout = run_python(&python, &program);
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        ["True", "True", "True", "True", "False"]
    );
}

#[test]
fn e2e_lists_compare_and_sort_lexicographically() {
    let Some(python) = python_cmd() else {
        eprintln!("skipping end-to-end check: no python interpreter found");
        return;
    };
    let program = pyfun::compile(
        "print ([1, 2] < [1, 3])\n\
         print ([1, 2, 3] < [1, 2])\n\
         print ([\"apple\"] < [\"banana\"])\n\
         print (List.sort [[2], [1], [1, 0]])",
    )
    .unwrap();
    let stdout = run_python(&python, &program);
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        ["True", "False", "True", "[[1], [1, 0], [2]]"]
    );
}

#[test]
fn e2e_boolean_logic() {
    let Some(python) = python_cmd() else {
        eprintln!("skipping end-to-end check: no python interpreter found");
        return;
    };
    let program = pyfun::compile(
        "let between lo hi x = lo <= x and x <= hi\n\
         print (true and not false)\n\
         print (1 < 2 or 5 < 3)\n\
         print (not (1 == 2))\n\
         print (between 0 10 5)\n\
         print (between 0 10 20)",
    )
    .unwrap();
    let stdout = run_python(&python, &program);
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        ["True", "True", "True", "True", "False"]
    );
}

#[test]
fn e2e_extern_calls_python() {
    run_and_check(
        "extern show: a -> string = str\n\
         extern ord: string -> int\n\
         extern pure tan: float -> float = math.tan\n\
         let label = show 42\n\
         let code = ord \"A\"\n\
         let root = tan 0.0",
        &[("label", "42"), ("code", "65"), ("root", "0.0")],
    );
}

#[test]
fn e2e_extern_import_calls_python() {
    // A declared `extern import` reaches a live call: `datetime.datetime
    // .fromisoformat` parses a fixed ISO string to a deterministic `datetime`
    // whose `str(...)` is stable.
    run_and_check(
        "extern import datetime\n\
         extern parse: string -> a = datetime.datetime.fromisoformat\n\
         let d = parse \"2020-01-02\"",
        &[("d", "2020-01-02 00:00:00")],
    );
}

#[test]
fn e2e_list_functions() {
    run_and_check(
        "let xs = [1, 2, 3, 4]\n\
         let doubled = List.map (fun x -> x * 2) xs\n\
         let small = List.filter (fun x -> x < 3) xs\n\
         let total = List.fold (fun a b -> a + b) 0 xs\n\
         let n = List.len xs\n\
         let s = List.sum xs\n\
         let r = List.rev xs\n\
         let q = List.range 1 4",
        &[
            ("doubled", "[2, 4, 6, 8]"),
            ("small", "[1, 2]"),
            ("total", "10"),
            ("n", "4"),
            ("s", "10"),
            ("r", "[4, 3, 2, 1]"),
            ("q", "[1, 2, 3]"),
        ],
    );
}

#[test]
fn e2e_set_functions() {
    run_and_check(
        "let s = Set.ofList [1, 2, 3, 3]\n\
         let n = Set.len s\n\
         let has = Set.contains 2 s\n\
         let u = Set.len (Set.union s (Set.ofList [3, 4]))\n\
         let i = Set.len (Set.intersect s (Set.ofList [3, 4]))\n\
         let d = Set.len (Set.difference s (Set.ofList [3, 4]))\n\
         let e = Set.len Set.empty",
        &[
            ("n", "3"),
            ("has", "True"),
            ("u", "4"),
            ("i", "1"),
            ("d", "2"),
            ("e", "0"),
        ],
    );
}

#[test]
fn int_literal_unified_to_float_lowers_to_a_float_literal() {
    // The `2` in a float list and the `1` in a float-typed `if` branch are
    // inferred `float`, so they must emit as Python floats (`2.0`, `1.0`) — a bare
    // `print` of them should show `2.0`, not `2`.
    let py = pyfun::compile(
        "let xs = [1.0, 2, 3.0]\n\
         let pick b = if b then 1 else 1.5",
    )
    .unwrap();
    assert!(py.contains("xs = [1.0, 2.0, 3.0]"), "{py}");
    assert!(py.contains("return 1.0"), "{py}");
    // A genuinely-int literal is untouched.
    let py2 = pyfun::compile("let n = 7\nlet m = n + 1").unwrap();
    assert!(py2.contains("n = 7\n") || py2.contains("n = 7"), "{py2}");
    assert!(!py2.contains("n = 7.0"), "{py2}");
}

#[test]
fn e2e_int_literal_unified_to_float_prints_as_float() {
    run_and_check(
        "let xs = [1.0, 2, 3.0]\n\
         let pick b = if b then 1 else 1.5\n\
         let r = pick true",
        &[("xs", "[1.0, 2.0, 3.0]"), ("r", "1.0")],
    );
}

#[test]
fn e2e_combinators() {
    run_and_check(
        "let sub a b = a - b\n\
         let i = id 42\n\
         let k = const 7 \"ignored\"\n\
         let f = flip sub 3 10\n\
         let mapped = List.map (const 0) [1, 2, 3]\n\
         let identity = List.map id [4, 5, 6]\n\
         let flipped = List.map (flip sub 100) [1, 2]",
        &[
            ("i", "42"),
            ("k", "7"),
            // flip sub 3 10 = sub 10 3 = 7
            ("f", "7"),
            ("mapped", "[0, 0, 0]"),
            ("identity", "[4, 5, 6]"),
            // flip sub 100 x = sub x 100 = x - 100
            ("flipped", "[-99, -98]"),
        ],
    );
}

#[test]
fn e2e_map_functions() {
    run_and_check(
        "let m = Map.add \"a\" 1 (Map.add \"b\" 2 Map.empty)\n\
         let found = Map.findOr \"a\" 0 m\n\
         let dflt = Map.findOr \"z\" 99 m\n\
         let sz = Map.len m\n\
         let mc = Map.contains \"b\" m\n\
         let rm = Map.len (Map.remove \"b\" m)\n\
         let ks = List.len (Map.keys m)\n\
         let hit = Option.withDefault 0 (Map.tryFind \"a\" m)\n\
         let miss = Option.withDefault 0 (Map.tryFind \"z\" m)",
        &[
            ("found", "1"),
            ("dflt", "99"),
            ("sz", "2"),
            ("mc", "True"),
            ("rm", "1"),
            ("ks", "2"),
            ("hit", "1"),
            ("miss", "0"),
        ],
    );
}

#[test]
fn e2e_adts_and_records_as_keys_and_elements() {
    // The generated `__hash__` lets ADTs and records be `Set` elements / `Map`
    // keys, with structural identity (equal values collapse).
    run_and_check(
        "type Color = Red | Green | Blue\n\
         type Point = { x: int, y: int }\n\
         let cs = Set.ofList [Red, Green, Red, Blue]\n\
         let ncolors = Set.len cs\n\
         let hasGreen = Set.contains Green cs\n\
         let m = Map.add (Some 1) \"one\" Map.empty\n\
         let v = Option.withDefault \"?\" (Map.tryFind (Some 1) m)\n\
         let pts = Set.ofList [Point { x = 1, y = 2 }, Point { x = 1, y = 2 }, Point { x = 3, y = 4 }]\n\
         let npts = Set.len pts",
        &[
            ("ncolors", "3"),
            ("hasGreen", "True"),
            ("v", "one"),
            ("npts", "2"),
        ],
    );
}

#[test]
fn e2e_seq_module_is_lazy() {
    // `Seq.range 0 1000000` then `take`/`map` forces only a handful of elements —
    // laziness, not a million-element materialization.
    run_and_check(
        "let nats = Seq.range 0 1000000\n\
         let squares = Seq.toList (Seq.take 5 (Seq.map (fun x -> x * x) nats))\n\
         let evens = Seq.toList (Seq.take 3 (Seq.filter (fun x -> x // 2 * 2 == x) nats))\n\
         let total = Seq.fold (fun acc x -> acc + x) 0 (Seq.ofList [1, 2, 3, 4])",
        &[
            ("squares", "[0, 1, 4, 9, 16]"),
            ("evens", "[0, 2, 4]"),
            ("total", "10"),
        ],
    );
}

#[test]
fn e2e_in_file_module() {
    run_and_check(
        "module Geometry =\n\
        \x20 let pi = 3\n\
        \x20 let area r = pi * r * r\n\
        \x20 let double a = area a + area a\n\
         let a = Geometry.area 10\n\
         let d = Geometry.double 2\n\
         let p = Geometry.pi",
        &[("a", "300"), ("d", "24"), ("p", "3")],
    );
}

#[test]
fn e2e_result_module() {
    run_and_check(
        "let safeDiv a b = if b == 0 then Error \"div0\" else Ok (a // b)\n\
         let r1 = safeDiv 10 2\n\
         let r2 = safeDiv 10 0\n\
         let a = Result.withDefault 0 (Result.map (fun x -> x + 1) r1)\n\
         let b = Result.withDefault 0 r2\n\
         let c = Result.isOk r1\n\
         let d = Result.isError r2\n\
         let e = Option.withDefault 0 (Result.toOption r1)\n\
         let f = Result.withDefault 0 (Result.bind (fun x -> safeDiv x 3) r1)\n\
         let g = Result.isError (Result.mapError (fun s -> s) r2)",
        &[
            ("a", "6"),
            ("b", "0"),
            ("c", "True"),
            ("d", "True"),
            ("e", "5"),
            ("f", "1"),
            ("g", "True"),
        ],
    );
}

#[test]
fn e2e_option_module() {
    run_and_check(
        "let some = Some 5\n\
         let none = None\n\
         let a = Option.withDefault 0 some\n\
         let b = Option.withDefault 0 none\n\
         let c = Option.isSome some\n\
         let d = Option.isNone none\n\
         let e = Option.map (fun x -> x + 1) some",
        &[
            ("a", "5"),
            ("b", "0"),
            ("c", "True"),
            ("d", "True"),
            ("e", "Some(6)"),
        ],
    );
}

// ---------- active patterns (`DESIGN.md` §7.2) ----------

#[test]
fn active_pattern_match_lowers_to_an_if_elif_chain() {
    let py = pyfun::compile(
        "let (|Even|Odd|) n = if n % 2 == 0 then Even else Odd\n\
         let f n =\n  match n:\n    case Even: \"e\"\n    case Odd: \"o\"",
    )
    .unwrap();
    // The recognizer is a plain def; the match tests its hoisted result.
    assert!(py.contains("def _ap_Even_Odd(n):"), "{py}");
    assert!(py.contains("if isinstance("), "{py}");
    assert!(py.contains("elif isinstance("), "{py}");
    // Hidden case classes are underscore-mangled.
    assert!(py.contains("class _Even:"), "{py}");
    assert!(py.contains("class _Odd:"), "{py}");
    // Evaluated once: the recognizer is called at exactly one site (plus its def).
    assert_eq!(py.matches("_ap_Even_Odd(").count(), 2, "{py}");
}

#[test]
fn bool_partial_lowers_to_a_truthiness_test() {
    let py = pyfun::compile(
        "let (|Blank|_|) s = s == \"\"\n\
         let f s =\n  match s:\n    case Blank: 1\n    case _: 0",
    )
    .unwrap();
    assert!(py.contains("def _ap_Blank(s):"), "{py}");
    // The hoisted bool result is the test itself — no isinstance.
    assert!(py.contains("if _pf_t0:"), "{py}");
    assert!(!py.contains("isinstance"), "{py}");
}

#[test]
fn distinct_parameterized_applications_hoist_separately() {
    let py = pyfun::compile(
        "let (|DivisibleBy|_|) d n = n % d == 0\n\
         let f n =\n  match n:\n    case DivisibleBy 3: 1\n    case DivisibleBy 5: 2\n    case _: 0",
    )
    .unwrap();
    // Two distinct applications (different arguments) → two hoisted calls + def.
    assert_eq!(py.matches("_ap_DivisibleBy(").count(), 3, "{py}");
    assert!(py.contains("_ap_DivisibleBy(3, n)"), "{py}");
    assert!(py.contains("_ap_DivisibleBy(5, n)"), "{py}");
}

#[test]
fn e2e_guarded_active_pattern_falls_through() {
    // A failing guard falls through to the next arm (return position).
    run_and_check(
        "let (|Even|Odd|) n = if n % 2 == 0 then Even else Odd\n\
         let describe n =\n  match n:\n    case Even if n > 100: \"big even\"\n    case Even: \"even\"\n    case Odd: \"odd\"\n\
         let a = describe 200\n\
         let b = describe 4\n\
         let c = describe 7",
        &[("a", "big even"), ("b", "even"), ("c", "odd")],
    );
}

#[test]
fn e2e_guarded_active_pattern_in_value_position() {
    // The match is a `let` value (then used), so the guarded lowering uses the
    // `_done` sentinel rather than an early `return`. A partial-Option guard binds.
    run_and_check(
        "let (|Positive|_|) n = if n > 0 then Some n else None\n\
         let classify n =\n  let label =\n    match n:\n      case Positive p if p > 100: \"big\"\n      case Positive p: \"small\"\n      case _: \"nonpos\"\n  String.concat label \"!\"\n\
         let a = classify 250\n\
         let b = classify 5\n\
         let c = classify 0",
        &[("a", "big!"), ("b", "small!"), ("c", "nonpos!")],
    );
}

#[test]
fn e2e_total_active_pattern() {
    run_and_check(
        "let (|Even|Odd|) n = if n % 2 == 0 then Even else Odd\n\
         let f n =\n  match n:\n    case Even: \"even\"\n    case Odd: \"odd\"\n\
         let a = f 4\n\
         let b = f 7",
        &[("a", "even"), ("b", "odd")],
    );
}

#[test]
fn e2e_total_active_pattern_with_fields() {
    run_and_check(
        "let (|Small|Big|) n = if n < 10 then Small n else Big (n - 10)\n\
         let f n =\n  match n:\n    case Small s: s\n    case Big b: b\n\
         let a = f 7\n\
         let b = f 25",
        &[("a", "7"), ("b", "15")],
    );
}

#[test]
fn e2e_option_partial_active_pattern() {
    run_and_check(
        "let (|Positive|_|) n = if n > 0 then Some n else None\n\
         let f n =\n  match n:\n    case Positive p: p\n    case _: 0\n\
         let a = f 7\n\
         let b = f (-3)",
        &[("a", "7"), ("b", "0")],
    );
}

#[test]
fn e2e_bool_partial_active_pattern() {
    run_and_check(
        "let (|Blank|_|) s = String.strip s == \"\"\n\
         let f s =\n  match s:\n    case Blank: \"empty\"\n    case _: \"text\"\n\
         let a = f \"   \"\n\
         let b = f \"hi\"",
        &[("a", "empty"), ("b", "text")],
    );
}

#[test]
fn e2e_parameterized_active_pattern_fizzbuzz() {
    run_and_check(
        "let (|DivisibleBy|_|) d n = n % d == 0\n\
         let fizz n =\n  match n:\n    case DivisibleBy 15: \"fizzbuzz\"\n    case DivisibleBy 3: \"fizz\"\n    case DivisibleBy 5: \"buzz\"\n    case _: String.fromInt n\n\
         let a = fizz 15\n\
         let b = fizz 9\n\
         let c = fizz 10\n\
         let d = fizz 7",
        &[("a", "fizzbuzz"), ("b", "fizz"), ("c", "buzz"), ("d", "7")],
    );
}

#[test]
fn e2e_active_pattern_side_effects_run_once_per_match() {
    // The recognizer prints; two arms use it, but the hoisted call runs once.
    let Some(python) = python_cmd() else {
        eprintln!("skipping end-to-end check: no python interpreter found");
        return;
    };
    let program = pyfun::compile(
        "let (|Tag|Untag|) n =\n  print \"eval\"\n  if n > 0 then Tag n else Untag\n\
         let r =\n  match 5:\n    case Tag v: v\n    case Untag: 0\n\
         print r",
    )
    .unwrap();
    let stdout = run_python(&python, &program);
    assert_eq!(stdout.replace("\r\n", "\n"), "eval\n5\n", "{program}");
}

#[test]
fn e2e_active_pattern_match_in_value_position() {
    // A match with active-pattern arms works as a top-level value binding too
    // (the chain assigns a temp instead of returning).
    run_and_check(
        "let (|Even|Odd|) n = if n % 2 == 0 then Even else Odd\n\
         let r =\n  match 6:\n    case Even: \"even\"\n    case Odd: \"odd\"",
        &[("r", "even")],
    );
}

#[test]
fn or_pattern_arm_lowers_to_an_or_of_isinstance_tests() {
    // `case Even | Odd:` becomes a disjunction of the alternatives' tests over a
    // *single* hoisted recognizer result (the memo collapses the shared call).
    let py = pyfun::compile(
        "let (|Even|Odd|) n = if n % 2 == 0 then Even else Odd\n\
         let f n =\n  match n:\n    case Even | Odd: \"eo\"",
    )
    .unwrap();
    assert!(
        py.contains("isinstance(_pf_t0, _Even) or isinstance(_pf_t0, _Odd)"),
        "{py}"
    );
    // The recognizer is called at exactly one site (plus its def), not once per
    // alternative.
    assert_eq!(py.matches("_ap_Even_Odd(").count(), 2, "{py}");
}

#[test]
fn structural_arm_lowers_to_a_one_armed_native_match() {
    // A constructor arm beside an AP arm emits a one-armed `match`/`case` (no
    // `case _`), so a non-match falls through to the next arm.
    let py = pyfun::compile(
        "let (|Answer|_|) o = o == Some 42\n\
         let f o =\n  match o:\n    case Answer: 1\n    case Some x: x\n    case None: 0",
    )
    .unwrap();
    // The AP arm still tests its hoisted recognizer result…
    assert!(py.contains("_pf_t0 = _ap_Answer(o)"), "{py}");
    assert!(py.contains("if _pf_t0:"), "{py}");
    // …and each structural arm is its own one-armed match, falling through.
    assert!(py.contains("match o:"), "{py}");
    assert!(py.contains("case Some(x):"), "{py}");
    assert!(py.contains("case None_():"), "{py}");
    // No exhaustive native `match` (which would carry a `case _`): the arms fall
    // through independently, backstopped by a `raise` at the end.
    assert!(!py.contains("case _:"), "{py}");
    assert!(
        py.contains("raise RuntimeError(\"non-exhaustive match\")"),
        "{py}"
    );
}

#[test]
fn structural_arm_beside_active_pattern_in_value_position() {
    // In value position the one-armed `match` is gated by the `_done` sentinel
    // (`if not _pf_t…:`) and assigns a temp instead of returning.
    let py = pyfun::compile(
        "let (|Answer|_|) o = o == Some 42\n\
         let classify o =\n  let label =\n    match o:\n      case Answer: \"answer\"\n      case Some n: String.fromInt n\n      case None: \"none\"\n  String.concat label \"!\"",
    )
    .unwrap();
    assert!(py.contains("if not _pf_t"), "{py}");
    assert!(py.contains("match o:"), "{py}");
    assert!(py.contains("case Some(n):"), "{py}");
}

#[test]
fn e2e_active_and_or_and_structural_arms_mix() {
    // One match combining an AP arm, an or-AP arm, and structural arms, with a
    // guard: an AP arm filters first (`Answer` beats `Some x` for `Some 42`), a
    // guarded structural arm that fails its guard falls through to a later arm,
    // and fall-through order is respected.
    run_and_check(
        "let (|Answer|_|) o = o == Some 42\n\
         let (|Even|Odd|) n = if n % 2 == 0 then Even else Odd\n\
         let describe o =\n  match o:\n    case Some x if x > 100: \"big\"\n    case Answer: \"the answer\"\n    case Some x: (match x:\n      case Even | Odd: String.fromInt x)\n    case None: \"none\"\n\
         let a = describe (Some 42)\n\
         let b = describe (Some 7)\n\
         let c = describe (Some 500)\n\
         let d = describe None",
        &[("a", "the answer"), ("b", "7"), ("c", "big"), ("d", "none")],
    );
}

#[test]
fn e2e_decode_primitives_and_totality() {
    // `Decode.decodeString` runs a decoder over JSON text totally: a good parse is
    // `Ok`, a type mismatch or malformed input is `Error` (here folded to a default).
    run_and_check(
        "let ok = Result.withDefault 0 (Decode.decodeString Decode.int \"42\")\n\
         let wrongType = Result.withDefault (0 - 1) (Decode.decodeString Decode.int \"\\\"x\\\"\")\n\
         let boolNotInt = Result.withDefault (0 - 1) (Decode.decodeString Decode.int \"true\")\n\
         let intAsFloat = Result.withDefault 0.0 (Decode.decodeString Decode.float \"3\")\n\
         let malformed = Result.isError (Decode.decodeString Decode.bool \"not json\")",
        &[
            ("ok", "42"),
            ("wrongType", "-1"),
            ("boolNotInt", "-1"),
            ("intAsFloat", "3.0"),
            ("malformed", "True"),
        ],
    );
}

#[test]
fn e2e_decode_record_via_map_and_field() {
    // The headline: decode a heterogeneous object into your own record, totally.
    run_and_check(
        "type Pair = { name: string, age: int }\n\
         let dec =\n  Decode.map2 (fun name age -> Pair { name = name, age = age })\n    (Decode.field \"name\" Decode.string)\n    (Decode.field \"age\" Decode.int)\n\
         let good =\n  match Decode.decodeString dec \"\"\"{\"name\": \"ada\", \"age\": 36}\"\"\":\n    case Ok p: p.name\n    case Error e: \"?\"\n\
         let missing = Result.isError (Decode.decodeString dec \"\"\"{\"name\": \"bob\"}\"\"\")",
        &[("good", "ada"), ("missing", "True")],
    );
}

#[test]
fn e2e_decode_list_nullable_oneof_andthen() {
    run_and_check(
        "let nums = Result.withDefault [] (Decode.decodeString (Decode.list Decode.int) \"[1, 2, 3]\")\n\
         let flex = Decode.oneOf [Decode.string, Decode.map String.fromInt Decode.int]\n\
         let fromStr = Result.withDefault \"?\" (Decode.decodeString flex \"\\\"hi\\\"\")\n\
         let fromInt = Result.withDefault \"?\" (Decode.decodeString flex \"7\")\n\
         let n = Result.withDefault (0 - 1) (Decode.decodeString (Decode.oneOf []) \"1\")",
        &[
            ("nums", "[1, 2, 3]"),
            ("fromStr", "hi"),
            ("fromInt", "7"),
            ("n", "-1"),
        ],
    );
}

#[test]
fn decode_pipeline_is_pure() {
    // Decoders are a pure sublanguage, so `let pure` over `decodeString` is accepted
    // (unlike a raw `json.loads` extern, which is `io` at the boundary).
    let py =
        pyfun::compile("let pure parse s = Decode.decodeString Decode.int s\nlet r = parse \"1\"")
            .unwrap();
    assert!(py.contains("_pf_dec_decode_string"), "{py}");
}

// ---------- interop cookbook examples (regression guard on the showcase) ----------
//
// Each `examples/interop/*.pyfun` is a public-facing showcase whose file comments
// commit to an exact stdout. These run the actual files end-to-end and pin that
// output, so a compiler change can't silently break the samples a newcomer reads.

#[test]
fn e2e_example_json_decode() {
    run_example("json_decode.pyfun", &["6.5", "0.0", "9.5", "0.0"]);
}

#[test]
fn e2e_example_json_to_adt() {
    run_example(
        "json_to_adt.pyfun",
        &[
            "ada (36): admin, dev",
            "decode failed (KeyError): 'roles'",
            "decode failed (ValueError): expected an int, got str",
            "decode failed (JSONDecodeError): Expecting value: line 1 column 1 (char 0)",
        ],
    );
}

#[test]
fn e2e_example_sqlite_query() {
    run_example("sqlite_query.pyfun", &["[(1, 1), (2, 4), (3, 9)]", "6"]);
}

#[test]
fn e2e_example_read_files() {
    run_example(
        "read_files.pyfun",
        &[
            "hello from pyfun",
            "(could not read no_such_file.txt: FileNotFoundError)",
        ],
    );
}

#[test]
fn e2e_example_http_fetch() {
    run_example(
        "http_fetch.pyfun",
        &["ok: hello pyfun", "failed: ValueError", "https /data.json"],
    );
}

#[test]
fn example_network_rail_compiles() {
    // The Network Rail example lives in a subdir with a Python helper + a bundled
    // sample and writes a file rather than printing, so it doesn't fit the stdout-
    // pinning `run_example` harness. Guard the part a compiler change can break:
    // its type-check and lowering.
    let path = format!(
        "{}/examples/interop/network-rail/chippenham.pyfun",
        env!("CARGO_MANIFEST_DIR")
    );
    let source =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
    pyfun::compile(&source)
        .unwrap_or_else(|e| panic!("network-rail chippenham.pyfun must compile: {e}"));
    // The Python-helper variant (same engine, nr_stream.py boundary) too.
    let fast = format!(
        "{}/examples/interop/network-rail/chippenham_fast.pyfun",
        env!("CARGO_MANIFEST_DIR")
    );
    let fast_src =
        std::fs::read_to_string(&fast).unwrap_or_else(|e| panic!("cannot read {fast}: {e}"));
    pyfun::compile(&fast_src)
        .unwrap_or_else(|e| panic!("network-rail chippenham_fast.pyfun must compile: {e}"));
}

// ---------- helpers ----------

/// Compile the interop example `name` from `examples/interop/`, run the emitted
/// Python, and assert its stdout is exactly `expected` (one entry per printed line).
/// Skipped (not failed) when no interpreter is on PATH, like the other e2e tests.
fn run_example(name: &str, expected: &[&str]) {
    let Some(python) = python_cmd() else {
        eprintln!("skipping {name} e2e: no python interpreter found");
        return;
    };
    let path = format!("{}/examples/interop/{name}", env!("CARGO_MANIFEST_DIR"));
    let source =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
    let program =
        pyfun::compile(&source).unwrap_or_else(|e| panic!("compiling {name} failed: {e}"));
    let stdout = run_python(&python, &program);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines, expected,
        "stdout drift in {name}\nfull stdout:\n{stdout}"
    );
}

/// Compile `source`, then run the emitted Python and assert that each named
/// top-level binding stringifies (`str(...)`) to the expected value.
fn run_and_check(source: &str, expected: &[(&str, &str)]) {
    let Some(python) = python_cmd() else {
        eprintln!("skipping end-to-end check: no python interpreter found");
        return;
    };
    let mut program =
        pyfun::compile(source).unwrap_or_else(|e| panic!("compile failed: {e}\n{source}"));
    program.push('\n');
    for (name, _) in expected {
        program.push_str(&format!("print({name})\n"));
    }
    let stdout = run_python(&python, &program);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.len(),
        expected.len(),
        "unexpected output\nprogram:\n{program}\nstdout:\n{stdout}"
    );
    for (line, (name, want)) in lines.iter().zip(expected) {
        assert_eq!(line, want, "binding `{name}` mismatch\nprogram:\n{program}");
    }
}

/// The first available Python interpreter command, if any.
fn python_cmd() -> Option<String> {
    for candidate in ["python", "python3"] {
        if Command::new(candidate).arg("--version").output().is_ok() {
            return Some(candidate.to_string());
        }
    }
    None
}

/// Run `program` by piping it to `python -` and return its stdout. Panics if the
/// interpreter reports an error, so compile bugs surface as test failures.
fn run_python(python: &str, program: &str) -> String {
    let mut child = Command::new(python)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn python");
    child
        .stdin
        .take()
        .expect("python stdin")
        .write_all(program.as_bytes())
        .expect("write program to python");
    let output = child.wait_with_output().expect("wait for python");
    assert!(
        output.status.success(),
        "python exited with {}\nprogram:\n{program}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("python stdout is utf-8")
}

// ---------- Tier-1 in-place linear fold (DESIGN.md §5) ----------
//
// Positive tests assert the loop form is emitted (`for ` present, `_pf_fold`
// absent) and, via e2e, that the value is correct. Adversarial tests assert the
// SAFE fallback is emitted (`_pf_fold` present, no in-place mutation) and still
// computes the copy-semantics value — these are the soundness net.

/// Assert `source` falls back to the `_pf_fold` helper (the fold-loop pass
/// rejected it), emits no in-place list mutation, and still computes `expected`.
fn assert_fold_fallback(source: &str, expected: &[(&str, &str)]) {
    let py = pyfun::compile(source).unwrap_or_else(|e| panic!("compile failed: {e}\n{source}"));
    assert!(
        py.contains("_pf_fold"),
        "expected the _pf_fold fallback (rejected fold), got:\n{py}"
    );
    assert!(
        !py.contains(".append(") && !py.contains(".extend("),
        "unexpected in-place mutation in a rejected fold:\n{py}"
    );
    run_and_check(source, expected);
}

#[test]
fn fold_opt_tuple_named_folder_lowers_to_loop() {
    let src = r#"type Cmd = Put int int | Push int | Skip
let step acc c =
  match acc:
    case (m, xs):
      match c:
        case Put k v: (Map.add k v m, xs)
        case Push n: (m, List.concat xs [n])
        case Skip: acc
let run cmds = List.fold step (Map.empty, []) cmds"#;
    let py = pyfun::compile(src).unwrap();
    assert!(py.contains("for c in cmds:"), "{py}");
    assert!(!py.contains("_pf_fold"), "{py}");
    assert!(py.contains("m[k] = v"), "{py}");
    assert!(py.contains("xs.append(n)"), "{py}");
    assert!(py.contains("return (m, xs)"), "{py}");
}

#[test]
fn e2e_fold_opt_tuple_named_folder() {
    let src = r#"type Cmd = Put int int | Push int | Skip
let step acc c =
  match acc:
    case (m, xs):
      match c:
        case Put k v: (Map.add k v m, xs)
        case Push n: (m, List.concat xs [n])
        case Skip: acc
let run cmds = List.fold step (Map.empty, []) cmds
let out = match run [Put 1 10, Push 2, Skip, Put 3 30, Push 4]: case (m, xs): (Map.toList m, xs)"#;
    let py = pyfun::compile(src).unwrap();
    assert!(!py.contains("_pf_fold"), "{py}");
    run_and_check(src, &[("out", "([(1, 10), (3, 30)], [2, 4])")]);
}

#[test]
fn fold_opt_single_map_lambda_lowers_to_subscript_assign() {
    let src = "let m = List.fold (fun m x -> Map.add x (x * 2) m) Map.empty [1, 2, 3]";
    let py = pyfun::compile(src).unwrap();
    assert!(py.contains("for x in [1, 2, 3]:"), "{py}");
    assert!(py.contains("m[x] ="), "{py}");
    assert!(!py.contains("_pf_fold"), "{py}");
    // The redundant `m = m` self-assign is suppressed.
    assert!(!py.contains("m = m"), "{py}");
}

#[test]
fn e2e_fold_opt_single_map_lambda() {
    let src = "let m = List.fold (fun m x -> Map.add x (x * 2) m) Map.empty [1, 2, 3]\n\
               let out = Map.toList m";
    run_and_check(src, &[("out", "[(1, 2), (2, 4), (3, 6)]")]);
}

#[test]
fn fold_opt_cross_slot_read_is_hoisted_before_mutation() {
    // P7: a later slot reads an earlier slot (`Map.len m`); the read must be
    // hoisted to a temp BEFORE the `m[x] = x` mutation so it sees the old value.
    let src = r#"let step acc x =
  match acc:
    case (m, log): (Map.add x x m, List.concat log [Map.len m])
let r = List.fold step (Map.empty, []) [1, 2, 3]"#;
    let py = pyfun::compile(src).unwrap();
    let temp = py.find("= len(m)").expect("hoisted len(m) temp");
    let mutate = py.find("m[x] = x").expect("m[x] = x mutation");
    assert!(
        temp < mutate,
        "read must be hoisted before the mutation:\n{py}"
    );
}

#[test]
fn e2e_fold_opt_cross_slot_read() {
    let src = r#"let step acc x =
  match acc:
    case (m, log): (Map.add x x m, List.concat log [Map.len m])
let r = List.fold step (Map.empty, []) [1, 2, 3]
let out = match r: case (m, log): (Map.toList m, log)"#;
    run_and_check(src, &[("out", "([(1, 1), (2, 2), (3, 3)], [0, 1, 2])")]);
}

#[test]
fn fold_opt_extend_form() {
    let src = "let acc = List.fold (fun acc x -> List.concat acc [x, x]) [] [1, 2]";
    let py = pyfun::compile(src).unwrap();
    assert!(py.contains(".extend("), "{py}");
    assert!(!py.contains("_pf_fold"), "{py}");
}

#[test]
fn e2e_fold_opt_extend_form() {
    let src = "let acc = List.fold (fun acc x -> List.concat acc [x, x]) [] [1, 2]\n\
               let out = acc";
    run_and_check(src, &[("out", "[1, 1, 2, 2]")]);
}

#[test]
fn e2e_fold_opt_seq_fold_and_piped_form() {
    let src = "let s = Seq.fold (fun m x -> Map.add x x m) Map.empty (Seq.range 0 3)\n\
               let p = [1, 2, 3] |> List.fold (fun a x -> List.concat a [x]) []\n\
               let so = Map.toList s";
    let py = pyfun::compile(src).unwrap();
    assert!(!py.contains("_pf_fold"), "both folds qualify:\n{py}");
    run_and_check(
        src,
        &[("so", "[(0, 0), (1, 1), (2, 2)]"), ("p", "[1, 2, 3]")],
    );
}

#[test]
fn fold_opt_nonatomic_key_is_hoisted() {
    // A non-atomic key expression is hoisted to a temp before the subscript
    // assignment (P7). (An effectful folder cannot typecheck against `List.fold`,
    // so the effect-ordering half of P7 is unreachable; this covers the value
    // dependency half.)
    let src = "let step m x = Map.add (x + 1) x m\n\
               let out = Map.toList (List.fold step Map.empty [1, 2, 3])";
    let py = pyfun::compile(src).unwrap();
    let temp = py.find("= x + 1").expect("hoisted key temp");
    let mutate = py.find("] = x").expect("subscript assign");
    assert!(temp < mutate, "key must hoist before the mutation:\n{py}");
    run_and_check(src, &[("out", "[(2, 1), (3, 2), (4, 3)]")]);
}

// ----- adversarial: each must fall back to `_pf_fold` and stay correct -----

#[test]
fn fold_named_init_binding_gets_a_defensive_copy() {
    // Tier B: a `Var` init now qualifies via a defensive shallow copy
    // (`m = dict(seed)`) — the loop mutates the copy, so a read of the
    // original *after* the fold still sees it untouched (the invariant the
    // old rejection protected).
    let src = "let seed = Map.empty\n\
               let grown = List.fold (fun m x -> Map.add x x m) seed [1, 2]\n\
               let a = Map.len seed\n\
               let b = Map.len grown";
    let py = pyfun::compile(src).unwrap();
    assert!(py.contains("dict(seed)"), "{py}");
    assert!(!py.contains("_pf_fold"), "{py}");
    run_and_check(src, &[("a", "0"), ("b", "2")]);
}

#[test]
fn fold_reject_component_stored_into_other_slot() {
    // A2: a slot stored into the other slot (retention) — snapshots must differ.
    let src = r#"let g acc x =
  match acc:
    case (names, log): (Map.add x x names, List.concat log [names])
let out = match List.fold g (Map.empty, []) [1, 2]: case (m, log): (Map.len m, List.map Map.len log)"#;
    assert_fold_fallback(src, &[("out", "(2, [0, 1])")]);
}

#[test]
fn fold_reject_swapped_slots() {
    // A3: slots returned swapped.
    let src = "let h acc x = match acc: case (a, b): (b, a)\n\
               let out = List.fold h (0, 100) [1, 2, 3]";
    assert_fold_fallback(src, &[("out", "(100, 0)")]);
}

#[test]
fn fold_reject_same_slot_twice() {
    // A4: one slot returned in two positions.
    let src = r#"let dup acc x = match acc: case (m, n): (Map.add x x m, m)
let out = match List.fold dup (Map.empty, Map.empty) [1, 2]: case (a, b): (Map.toList a, Map.toList b)"#;
    assert_fold_fallback(src, &[("out", "([(1, 1), (2, 2)], [(1, 1)])")]);
}

#[test]
fn fold_reject_closure_capture() {
    // A5: a slot captured in a closure stored into the accumulator.
    let src = r#"let cap acc x =
  match acc:
    case (m, fs): (Map.add x x m, List.concat fs [fun u -> Map.len m])
let out = match List.fold cap (Map.empty, []) [1, 2]: case (m, fs): List.map (fun f -> f 0) fs"#;
    let py = pyfun::compile(src).unwrap();
    assert!(py.contains("_pf_fold"), "{py}");
    run_and_check(src, &[("out", "[0, 1]")]);
}

#[test]
fn fold_reject_acc_escapes_to_user_function() {
    // A6: the accumulator is passed to a user function (the sortLegs shape).
    let src = "let ins x acc = List.concat [x] acc\n\
               let rev xs = List.fold (fun acc x -> ins x acc) [] xs\n\
               let out = rev [1, 2, 3]";
    assert_fold_fallback(src, &[("out", "[3, 2, 1]")]);
}

#[test]
fn fold_reject_acc_in_nontail_nonwhitelisted_position() {
    // A7: the accumulator used as a non-whitelisted read (a `List.contains` needle).
    let src = "let k acc x = if List.contains acc [[]] then acc else List.concat acc [x]\n\
               let out = List.fold k [] [1, 2, 3]";
    assert_fold_fallback(src, &[("out", "[]")]);
}

#[test]
fn fold_reject_fresh_reset_slot() {
    // A8: a slot reset to a fresh container mid-fold (punted).
    let src = r#"let rf acc x = match acc: case (m, n): if x == 0 then (Map.empty, n) else (Map.add x x m, n)
let out = match List.fold rf (Map.empty, 0) [0, 1, 2]: case (m, n): (Map.toList m, n)"#;
    assert_fold_fallback(src, &[("out", "([(1, 1), (2, 2)], 0)")]);
}

#[test]
fn fold_reject_free_var_capture_at_site() {
    // A9: a named folder's free var is shadowed by a call-site local.
    let src = "let base = 10\n\
               let addb acc x = List.concat acc [x + base]\n\
               let usefn n =\n  \
                 let base = 99\n  \
                 List.fold addb [] [1]\n\
               let out = usefn 0";
    assert_fold_fallback(src, &[("out", "[11]")]);
}

#[test]
fn fold_reject_wildcard_slot() {
    // A wildcard destructure slot has no name to thread.
    let src = r#"let w acc x = match acc: case (m, _): (Map.add x x m, Map.empty)
let out = match List.fold w (Map.empty, Map.empty) [1, 2]: case (a, b): (Map.toList a, Map.toList b)"#;
    assert_fold_fallback(src, &[("out", "([(1, 1), (2, 2)], [])")]);
}

#[test]
fn fold_reject_inside_in_file_module() {
    // Name mangling inside an in-file module would apply inconsistently (P8).
    let src = "module M =\n  \
                 let scan xs = List.fold (fun m x -> Map.add x x m) Map.empty xs\n\
               let r = Map.len (M.scan [1, 2, 3])";
    assert_fold_fallback(src, &[("r", "3")]);
}

#[test]
fn fold_reject_partial_application() {
    // A partially-applied fold (2 args) is not fully applied — falls through.
    let src = "let f a b = a + b\n\
               let g = List.fold f\n\
               let out = g 0 [1, 2, 3]";
    assert_fold_fallback(src, &[("out", "6")]);
}

#[test]
fn fold_reject_folder_is_a_parameter() {
    // The folder is a parameter (no inlinable body).
    let src = "let foldWith f = List.fold f Map.empty [1, 2, 3]\n\
               let out = Map.len (foldWith (fun m x -> Map.add x x m))";
    assert_fold_fallback(src, &[("out", "3")]);
}

// ----- Tier B: block-local named folders -----

#[test]
fn fold_local_named_folder_block_lowers_to_loop() {
    // A block-local `let step acc x = …` folder (its `def` and the fold share one
    // Python frame) inlines to an in-place loop, just like a top-level folder.
    let src = "let build xs =\n  \
                 let step acc x = Map.add x (x * 2) acc\n  \
                 List.fold step Map.empty xs\n\
               let out = Map.toList (build [1, 2, 3])";
    let py = pyfun::compile(src).unwrap();
    assert!(py.contains("for x in xs:"), "{py}");
    assert!(py.contains("acc[x] ="), "{py}");
    assert!(!py.contains("_pf_fold"), "{py}");
    run_and_check(src, &[("out", "[(1, 2), (2, 4), (3, 6)]")]);
}

#[test]
fn fold_local_named_folder_dedup_shape_lowers_to_loop() {
    // The dedupLegs shape (examples/interop/network-rail/chippenham.pyfun): a
    // block-local tuple-accumulator folder whose arm opens a block with a plain
    // value `let k = …` (a whitelisted read of `seen` via `Map.tryFind`), storing
    // the fresh element into the list slot.
    let src = "type Leg = { depMin: int, dest: string }\n\
               let dedupLegs legs =\n  \
                 let step acc leg =\n    \
                   match acc:\n      \
                     case (seen, out):\n        \
                       let k = f\"{leg.depMin}|{leg.dest}\"\n        \
                       match Map.tryFind k seen:\n          \
                         case Some _: (seen, out)\n          \
                         case None: (Map.add k true seen, List.concat out [leg])\n  \
                 match List.fold step (Map.empty, []) legs:\n    \
                   case (_, out): out\n\
               let out = List.map (fun l -> l.depMin) (dedupLegs [Leg { depMin = 1, dest = \"a\" }, Leg { depMin = 1, dest = \"a\" }, Leg { depMin = 2, dest = \"b\" }])";
    let py = pyfun::compile(src).unwrap();
    assert!(py.contains("for leg in legs:"), "{py}");
    assert!(py.contains("seen[k] = True"), "{py}");
    assert!(py.contains("out.append(leg)"), "{py}");
    assert!(!py.contains("_pf_fold"), "{py}");
    run_and_check(src, &[("out", "[1, 2]")]);
}

// ----- Tier B: registry shadow-coherence (the folder name is rebound) -----

#[test]
fn fold_shadow_rebound_local_folder_uses_newest_body() {
    // Rebinding `let step … let step …` in one block: the fold must inline the
    // NEWEST body (`x * 2`), not the stale one (`x + 100`). Differential: the
    // emitted loop carries the newest constant, and the value is the newest's.
    let src = "let build xs =\n  \
                 let step acc x = List.concat acc [x + 100]\n  \
                 let step acc x = List.concat acc [x * 2]\n  \
                 List.fold step [] xs\n\
               let out = build [1, 2, 3]";
    let py = pyfun::compile(src).unwrap();
    assert!(!py.contains("_pf_fold"), "{py}");
    // The dead `def step`s carry both constants; the LOOP (after the `for`) must
    // carry only the newest body's `x * 2`.
    let loop_body = &py[py.find("for x in xs:").expect("emitted loop")..];
    assert!(loop_body.contains("x * 2"), "loop uses newest body:\n{py}");
    assert!(
        !loop_body.contains("x + 100"),
        "loop must not use the stale body:\n{py}"
    );
    run_and_check(src, &[("out", "[2, 4, 6]")]);
}

#[test]
fn fold_shadow_lambda_param_uses_parameter() {
    // A lambda parameter named like an outer local folder shadows it: the fold
    // uses the parameter (a runtime function value, so it falls back to `_pf_fold`)
    // — never the stale outer `step` body. Correctness via the reduce value.
    let src = "let build combiner xs =\n  \
                 let step acc x = List.concat acc [x + 100]\n  \
                 (fun step -> List.fold step [] xs) combiner\n\
               let out = build (fun acc x -> List.concat acc [x * 2]) [1, 2, 3]";
    let py = pyfun::compile(src).unwrap();
    assert!(
        py.contains("_pf_fold"),
        "parameter folder is not inlinable:\n{py}"
    );
    run_and_check(src, &[("out", "[2, 4, 6]")]);
}

#[test]
fn fold_shadow_match_binder_uses_binding() {
    // A match-arm binder named like an outer local folder shadows it: the fold
    // uses the binding, never the stale outer `step` body. Correctness via reduce.
    let src = "let build sel xs =\n  \
                 let step acc x = List.concat acc [x + 100]\n  \
                 match sel:\n    \
                   case Some step: List.fold step [] xs\n    \
                   case None: []\n\
               let out = build (Some (fun acc x -> List.concat acc [x * 2])) [1, 2, 3]";
    let py = pyfun::compile(src).unwrap();
    assert!(
        py.contains("_pf_fold"),
        "bound folder is not inlinable:\n{py}"
    );
    run_and_check(src, &[("out", "[2, 4, 6]")]);
}

// ----- Tier B: chained updates in one slot -----

#[test]
fn fold_chained_map_add_updates_one_slot_inner_first() {
    // `Map.add k2 v2 (Map.add k1 v1 m)` → `m[k1]=v1` then `m[k2]=v2` — the inner
    // add is emitted first, matching the value the copy chain would build.
    let src = "let step m x = Map.add (x * 10) x (Map.add x x m)\n\
               let out = Map.toList (List.fold step Map.empty [1, 2])";
    let py = pyfun::compile(src).unwrap();
    assert!(!py.contains("_pf_fold"), "{py}");
    let inner = py.find("m[x] = x").expect("inner add m[x] = x");
    let outer = py.find("m[_pf_t0] = x").expect("outer add m[x * 10] = x");
    assert!(inner < outer, "inner add must precede the outer:\n{py}");
    run_and_check(src, &[("out", "[(1, 1), (10, 1), (2, 2), (20, 2)]")]);
}

// ----- Tier B: store-then-reset (the batching idiom) -----

#[test]
fn fold_reset_store_batching_idiom() {
    // `([], List.concat groups [cur])` — the `cur` slot resets to a fresh `[]` and
    // its OLD object is stored into `groups`. The old reference must be
    // force-hoisted to a temp BEFORE the reset rebinds `cur`.
    let src = "let step acc x =\n  \
                 match acc:\n    \
                   case (cur, groups):\n      \
                     if x == 0 then ([], List.concat groups [cur])\n      \
                     else (List.concat cur [x], groups)\n\
               let out = match List.fold step ([], []) [1, 2, 0, 3, 0]: case (cur, groups): (cur, groups)";
    let py = pyfun::compile(src).unwrap();
    assert!(!py.contains("_pf_fold"), "{py}");
    let hoist = py.find("= cur").expect("force-hoisted old cur reference");
    let reset = py.find("cur = _pf_t0").expect("cur reset to a fresh slot");
    assert!(
        hoist < reset,
        "old cur must be hoisted before the reset:\n{py}"
    );
    assert!(
        py.contains("groups.append("),
        "old cur appended to groups:\n{py}"
    );
    run_and_check(src, &[("out", "([], [[1, 2], [3]])")]);
}

// ----- Tier B: Map.remove / Set.remove in chains -----

#[test]
fn fold_map_remove_in_chain_lowers_to_pop() {
    // `Map.remove k (Map.add …)` → `m[…] = …` then `m.pop(k, None)` (mirrors
    // `_pf_map_remove`), inner-first.
    let src = "let step m x = Map.remove x (Map.add (x + 1) x m)\n\
               let out = Map.toList (List.fold step Map.empty [1, 2, 3])";
    let py = pyfun::compile(src).unwrap();
    assert!(!py.contains("_pf_fold"), "{py}");
    let add = py.find("m[_pf_t0] = x").expect("inner add");
    let pop = py.find("m.pop(x, None)").expect("Map.remove -> pop");
    assert!(add < pop, "add must precede the remove:\n{py}");
    run_and_check(src, &[("out", "[(4, 3)]")]);
}

#[test]
fn fold_set_remove_in_chain_lowers_to_discard() {
    // `Set.remove x (Set.add …)` → `s.add(…)` then `s.discard(x)` (mirrors
    // `_pf_set_remove`), inner-first.
    let src = "let step s x = Set.remove (x - 1) (Set.add x s)\n\
               let out = Set.toList (List.fold step Set.empty [1, 2, 3])";
    let py = pyfun::compile(src).unwrap();
    assert!(!py.contains("_pf_fold"), "{py}");
    let add = py.find("s.add(x)").expect("inner Set.add");
    let discard = py.find("s.discard(").expect("Set.remove -> discard");
    assert!(add < discard, "add must precede the remove:\n{py}");
    run_and_check(src, &[("out", "[3]")]);
}

// ----- Tier B: `Var` inits (defensive copy vs alias) -----

#[test]
fn fold_var_init_defensive_copy_dict_list_set() {
    // Each mutated-in-place slot with a named (`Var`) init binds a defensive
    // shallow copy whose constructor is inferred from its op family
    // (`dict`/`list`/`set`); the originals read unchanged after the fold.
    let src = "let step acc x =\n  \
                 match acc:\n    \
                   case (m, l, s): (Map.add x x m, List.concat l [x], Set.add x s)\n\
               let dseed = Map.empty\n\
               let lseed = []\n\
               let sseed = Set.empty\n\
               let r = List.fold step (dseed, lseed, sseed) [1, 2]\n\
               let out = match r: case (m, l, s): (Map.toList m, l, Set.toList s)\n\
               let origs = (Map.len dseed, List.len lseed, Set.len sseed)";
    let py = pyfun::compile(src).unwrap();
    assert!(!py.contains("_pf_fold"), "{py}");
    assert!(py.contains("dict(dseed)"), "dict copy:\n{py}");
    assert!(py.contains("list(lseed)"), "list copy:\n{py}");
    assert!(py.contains("set(sseed)"), "set copy:\n{py}");
    run_and_check(
        src,
        &[
            ("out", "([(1, 1), (2, 2)], [1, 2], [1, 2])"),
            ("origs", "(0, 0, 0)"),
        ],
    );
}

#[test]
fn fold_var_init_alias_for_passthrough_slot() {
    // A pass-through slot (never mutated in place) with a `Var` init binds as a
    // plain alias — no defensive copy constructor.
    let src = "let step acc x =\n  \
                 match acc:\n    \
                   case (m, keep): (Map.add x x m, keep)\n\
               let seed = Map.empty\n\
               let seed2 = Map.empty\n\
               let r = List.fold step (seed, seed2) [1, 2]\n\
               let out = match r: case (m, keep): (Map.len m, Map.len keep, Map.len seed2)";
    let py = pyfun::compile(src).unwrap();
    assert!(!py.contains("_pf_fold"), "{py}");
    assert!(py.contains("m = dict(seed)"), "mutated slot copies:\n{py}");
    assert!(
        py.contains("keep = seed2"),
        "pass-through slot aliases:\n{py}"
    );
    assert!(
        !py.contains("dict(seed2)") && !py.contains("list(seed2)") && !py.contains("set(seed2)"),
        "alias slot must not be copied:\n{py}"
    );
    run_and_check(src, &[("out", "(2, 0, 0)")]);
}

// ----- Tier B: soundness fix — parameterized local `let` closes over the accumulator -----

#[test]
fn fold_reject_parameterized_local_let_closure() {
    // A *parameterized* local `let peek u = Map.len acc` is a deferred closure over
    // the accumulator: it could observe a future mutation, so the fold is rejected
    // (a plain value `let` with a whitelisted read still qualifies — see the dedup
    // shape above).
    let src = "let step acc x =\n  \
                 let peek u = Map.len acc\n  \
                 Map.add x (peek 0) acc\n\
               let out = Map.toList (List.fold step Map.empty [1, 2, 3])";
    assert_fold_fallback(src, &[("out", "[(1, 0), (2, 1), (3, 2)]")]);
}
