//! End-to-end tests for the multi-file driver's filesystem entry point
//! (`project::build_from_path`). The in-memory graph/cycle/topo logic is unit
//! tested in `src/project/mod.rs`; here we exercise the real
//! `<root>/<name>.pyfun` resolution against temp files on disk.

use std::fs;
use std::path::PathBuf;

use pyfun::project::{self, ProjectError};

/// A unique scratch directory for one test, cleaned up on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("pyfun_project_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn write(&self, file: &str, source: &str) {
        fs::write(self.0.join(file), source).unwrap();
    }

    fn path(&self, file: &str) -> PathBuf {
        self.0.join(file)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn resolves_a_two_file_project_from_disk() {
    let dir = Scratch::new("two_file");
    dir.write("main.pyfun", "import Geometry\nlet a = Geometry.area 2 3");
    dir.write("geometry.pyfun", "let area w h = w * h");

    let project = project::build_from_path(&dir.path("main.pyfun")).unwrap();
    let names: Vec<&str> = project.modules.iter().map(|m| m.name.as_str()).collect();
    // Dependency first, entry last.
    assert_eq!(names, ["Geometry", "Main"]);
    assert_eq!(project.entry().name, "Main");
}

#[test]
fn a_missing_imported_file_is_reported() {
    let dir = Scratch::new("missing");
    dir.write("main.pyfun", "import Geometry\nlet x = 1");

    let err = project::build_from_path(&dir.path("main.pyfun")).unwrap_err();
    match err {
        ProjectError::Missing { name, importer } => {
            assert_eq!(name, "Geometry");
            assert_eq!(importer.as_deref(), Some("Main"));
        }
        other => panic!("expected a missing-module error, got {other:?}"),
    }
}
