//! The `exn` effect label (`DESIGN.md` §4, §6): a call that may raise says so,
//! and `try` is where that stops.

/// The displayed type of the top-level binding `name` in `source`.
fn type_of(source: &str, name: &str) -> String {
    let analysis = pyfun::analyze(source);
    assert!(
        analysis.diagnostics.is_empty(),
        "type-checks: {:?}\n{source}",
        analysis.diagnostics
    );
    let module = analysis.module.as_ref().expect("parses");
    let span = module
        .items
        .iter()
        .find_map(|item| match item {
            pyfun::syntax::Item::Let(binding) if binding.name() == Some(name) => {
                Some(binding.target_span.span())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no top-level `{name}`"));
    analysis
        .types
        .iter()
        .find(|t| t.span == span)
        .map(|t| t.ty.clone())
        .unwrap_or_else(|| panic!("no type recorded for `{name}`"))
}

fn first_error(source: &str) -> String {
    pyfun::analyze(source)
        .diagnostics
        .first()
        .map(|e| e.message.clone())
        .unwrap_or_else(|| panic!("expected an error in:\n{source}"))
}

const PARSE: &str = "extern pure to_number : str ->{exn} int = int\n";
const READ: &str =
    "extern read_text : str ->{io, exn} str = pathlib.Path.read_text\nextern import pathlib\n";

#[test]
fn an_extern_that_says_so_may_raise() {
    // An arrow that names its effects is taken as written, as it always has
    // been: `exn` alone for a conversion that can fail, `io` beside it for a
    // file that may not be there.
    assert_eq!(
        type_of(&format!("{PARSE}let parse text = to_number text"), "parse"),
        "str ->{exn} int"
    );
    assert_eq!(
        type_of(&format!("{READ}let load path = read_text path"), "load"),
        "str ->{io, exn} str"
    );
}

#[test]
fn it_flows_through_callers_and_combinators() {
    let source = format!(
        "{PARSE}let parse_all texts = List.map to_number texts\nlet total texts = List.sum (parse_all texts)"
    );
    assert_eq!(type_of(&source, "parse_all"), "List str ->{exn} List int");
    assert_eq!(type_of(&source, "total"), "List str ->{exn} int");
}

#[test]
fn try_is_where_it_stops() {
    let source = format!(
        "{PARSE}let safe text = try (to_number text)\nlet safe_all texts = try (List.map to_number texts)"
    );
    assert_eq!(type_of(&source, "safe"), "str -> Result int Exception");
    assert_eq!(
        type_of(&source, "safe_all"),
        "List str -> Result (List int) Exception"
    );
}

#[test]
fn try_stops_only_exn() {
    // Reading a file is still `io` after the exception has been caught.
    let source = format!("{READ}let load path = try (read_text path)");
    assert_eq!(type_of(&source, "load"), "str ->{io} Result str Exception");
}

#[test]
fn a_function_built_inside_try_keeps_its_own_exn() {
    // `try` catches what is raised while its body runs. A function it hands
    // back runs later, outside it.
    let source = format!("{PARSE}let later _ = try (fun text -> to_number text)");
    assert_eq!(
        type_of(&source, "later"),
        "a -> Result (str ->{exn} int) Exception"
    );
}

#[test]
fn let_pure_holds_a_function_to_not_raising() {
    let failing = format!("{PARSE}let pure parse text = to_number text");
    let message = first_error(&failing);
    assert!(message.contains("exn"), "{message}");
    // Caught, it is pure again.
    let caught = format!("{PARSE}let pure parse text = try (to_number text)");
    assert_eq!(type_of(&caught, "parse"), "str -> Result int Exception");
}

#[test]
fn a_declared_arrow_can_ask_for_a_callback_that_does_not_raise() {
    // A field typed with a plain arrow takes a function that performs nothing,
    // so one that may raise does not fit it.
    let source = format!("{PARSE}type Reader = Reader(str -> int)\nlet r = Reader to_number");
    let message = first_error(&source);
    assert!(message.contains("exn"), "{message}");
    // The arrow can say that it may.
    let fits = format!("{PARSE}type Reader = Reader(str ->{{exn}} int)\nlet r = Reader to_number")
        .replace("{{", "{")
        .replace("}}", "}");
    assert_eq!(type_of(&fits, "r"), "Reader");
}

#[test]
fn async_catch_stops_it_for_what_is_awaited() {
    let source = "extern fetch : str ->{io, exn} Async str = aiohttp.get\nextern import aiohttp\nlet get url = Async.catch (fetch url)\nlet raw url = fetch url";
    assert_eq!(
        type_of(source, "get"),
        "str ->{io} Async (Result str Exception)"
    );
    assert_eq!(type_of(source, "raw"), "str ->{io, exn} Async str");
}

#[test]
fn an_unknown_label_names_the_ones_there_are() {
    let message = first_error("type Handler = H(str ->{raises} int)");
    assert!(message.contains("`io`, `async`, `exn`"), "{message}");
}

#[test]
fn the_label_is_erased_from_the_emitted_python() {
    let python = pyfun::compile(&format!(
        "{PARSE}let parse text = to_number text\nprint (try (parse \"12\"))"
    ))
    .expect("compiles");
    assert!(!python.contains("exn"), "{python}");
    assert!(
        python.contains("def parse(text):\n    return int(text)\n"),
        "{python}"
    );
}
