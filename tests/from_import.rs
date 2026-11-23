//! `from Module import a, B` (`DESIGN.md` §6.1): the names a file asks a module
//! for, written bare.

use pyfun::project;

const GEOMETRY: (&str, &str) = (
    "Geometry",
    "type Shape = Circle(float) | Rect { width: float, height: float }\n\
     type Pt = { x: int, y: int }\n\
     let pi = 3.0\n\
     let area shape =\n    match shape:\n        case Circle(r): pi * r * r\n        case Rect { width, height }: width * height\n",
);

fn build(main: &str) -> project::Project {
    let files = vec![
        ("Main".to_string(), main.to_string()),
        (GEOMETRY.0.to_string(), GEOMETRY.1.to_string()),
        (
            "Other".to_string(),
            "let area side = side * side\nlet half x = x / 2.0\n".to_string(),
        ),
    ];
    project::build("Main", move |name| {
        files
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, s)| s.clone())
    })
    .expect("the project resolves")
}

/// The emitted `main.py` of a project that type-checks.
fn main_py(main: &str) -> String {
    let project = build(main);
    let (errors, _) = project::check_reporting(&project);
    assert!(
        errors.is_empty(),
        "type-checks: {:?}",
        errors.iter().flat_map(|m| &m.errors).collect::<Vec<_>>()
    );
    project::compile(&project)
        .expect("lowers")
        .files
        .into_iter()
        .find(|(name, _)| name == "main.py")
        .expect("main.py")
        .1
}

/// The errors of `Main`, as messages.
fn main_errors(main: &str) -> Vec<String> {
    let project = build(main);
    project::check_reporting(&project)
        .0
        .into_iter()
        .filter(|m| m.name == "Main")
        .flat_map(|m| m.errors)
        .map(|e| e.message)
        .collect()
}

/// The warnings of `Main`, as messages.
fn main_warnings(main: &str) -> Vec<String> {
    let project = build(main);
    let (errors, warned) = project::check_reporting(&project);
    assert!(errors.is_empty(), "{errors:?}");
    warned
        .into_iter()
        .filter(|m| m.name == "Main")
        .flat_map(|m| m.warnings)
        .map(|w| w.message)
        .collect()
}

#[test]
fn it_parses_and_prints_back() {
    let source = "from Geometry import area, Circle";
    assert_eq!(pyfun::format(source).unwrap().trim_end(), source);
    // The plain form is untouched.
    assert_eq!(
        pyfun::format("import Geometry").unwrap().trim_end(),
        "import Geometry"
    );
    // `from` is a word only ahead of a module name and `import`.
    assert!(pyfun::parse("let from = 1\nlet to = from + 1").is_ok());
}

#[test]
fn an_imported_name_is_written_bare_and_emitted_through_its_module() {
    let python = main_py(
        "from Geometry import area, Circle, Rect, Pt\n\
         print (area (Circle 1.0))\n\
         print (area (Rect { width = 2.0, height = 3.0 }))\n\
         print (Pt { x = 1, y = 2 })\n",
    );
    assert_eq!(
        python,
        "import geometry\n\
         print(geometry.area(geometry.Circle(1.0)))\n\
         print(geometry.area(geometry.Rect(width=2.0, height=3.0)))\n\
         print(geometry.Pt(1, 2))\n"
    );
}

#[test]
fn an_imported_constructor_is_matched_bare() {
    let python = main_py(
        "from Geometry import Circle, Rect\n\
         let describe shape =\n    match shape:\n        case Circle(r): f\"circle {r}\"\n        case Rect { width, height }: f\"rect {width}x{height}\"\n",
    );
    assert!(python.contains("case geometry.Circle(r):"), "{python}");
    assert!(
        python.contains("case geometry.Rect(width=width, height=height):"),
        "{python}"
    );
}

#[test]
fn a_local_name_shadows_the_import() {
    // A parameter, a `let`, a pattern variable and a lambda's parameter each
    // mean themselves; past them the name is the import again.
    let python = main_py(
        "from Geometry import area, pi\n\
         let double area = area * 2\n\
         let local _ =\n    let pi = 4.0\n    pi\n\
         let pick opt =\n    match opt:\n        case Some(area): area\n        case None: 0.0\n\
         let all = List.map (fun pi -> pi + 1.0) [pi]\n\
         let shape_area shape = area shape\n",
    );
    assert!(
        python.contains("def double(area):\n    return area * 2\n"),
        "{python}"
    );
    assert!(python.contains("    pi = 4.0\n    return pi\n"), "{python}");
    assert!(
        python.contains("lambda pi: pi + 1.0, [geometry.pi]"),
        "{python}"
    );
    assert!(
        python.contains("def shape_area(shape):\n    return geometry.area(shape)\n"),
        "{python}"
    );
}

#[test]
fn it_binds_the_names_and_not_the_module() {
    let errors = main_errors("from Geometry import area\nlet x = Geometry.pi\n");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("`Geometry` is not imported as a module")
            && errors[0].contains("Add `import Geometry`"),
        "{errors:?}"
    );
    // With both lines, both forms.
    let python = main_py(
        "from Geometry import area\nimport Geometry\nprint (area (Geometry.Circle Geometry.pi))\n",
    );
    assert!(
        python.contains("print(geometry.area(geometry.Circle(geometry.pi)))"),
        "{python}"
    );
}

#[test]
fn a_name_means_one_thing() {
    let errors = main_errors("from Geometry import area\nlet area x = x\n");
    assert!(
        errors
            .iter()
            .any(|e| e.contains("`area` is imported from `Geometry` and also defined in this file")),
        "{errors:?}"
    );
    let errors = main_errors("from Geometry import area, area\nprint area\n");
    assert!(
        errors
            .iter()
            .any(|e| e.contains("`area` is imported from `Geometry` twice")),
        "{errors:?}"
    );
    let errors = main_errors("from Geometry import area\nfrom Other import area\nprint area\n");
    assert!(
        errors
            .iter()
            .any(|e| e.contains("`area` is imported from both `Geometry` and `Other`")),
        "{errors:?}"
    );
}

#[test]
fn a_name_the_module_does_not_have_is_an_error() {
    let errors = main_errors("from Geometry import areaa\n");
    assert_eq!(errors, vec!["module `Geometry` has no `areaa` to import"]);
}

#[test]
fn a_star_is_refused_and_a_python_module_is_sent_to_extern() {
    let star = pyfun::parse("from Geometry import *")
        .unwrap_err()
        .message();
    assert!(star.contains("name what you import"), "{star}");
    let python = pyfun::parse("from math import sqrt").unwrap_err().message();
    assert!(python.contains("`extern import module`"), "{python}");
}

#[test]
fn a_name_nothing_uses_is_a_warning() {
    let warnings = main_warnings("from Geometry import area, pi\nprint pi\n");
    assert_eq!(warnings, vec!["`area` (from `Geometry`) is never used"]);
    // A type is used by being written in a type.
    let warnings = main_warnings(
        "from Geometry import Shape, area\ntype Scene = Scene(List Shape)\nlet total shapes = List.map area shapes\n",
    );
    assert_eq!(warnings, Vec::<String>::new());
}

#[test]
fn the_editor_follows_a_bare_name_to_its_module() {
    // The resolver reads the file as written out in full, so a bare use and
    // the name in the import's own list are both references to the member.
    let module = pyfun::parse("from Geometry import area\nlet total s = area s\n").unwrap();
    let refs: Vec<(String, String, usize)> = pyfun::lsp::resolve::qualified_references(&module)
        .into_iter()
        .map(|q| (q.module, q.member, q.span.start))
        .collect();
    assert_eq!(
        refs,
        vec![
            ("Geometry".to_string(), "area".to_string(), 21),
            ("Geometry".to_string(), "area".to_string(), 40),
        ]
    );
}
