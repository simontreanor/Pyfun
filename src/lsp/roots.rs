//! Where a document's project begins, and the files in it (`DESIGN.md` §6.1, §9).
//!
//! The compiler is told which file is being run, and reads every import from
//! that file's folder. The editor is told only which file is open, which may
//! be a module deep in a package, so the top of the project is worked out from
//! what the files say. Two things place it above the document's own folder:
//!
//! - **what the document imports**: `import Shapes.Util` in
//!   `shapes/geometry.pyfun` is found one folder up, so that is where the
//!   document's imports are read from;
//! - **who imports the document**: a file above that says `import
//!   Shapes.Geometry`, and means this one, makes its folder the top even when
//!   the document imports nothing itself.
//!
//! The furthest of the two is the top. Every cross-file answer (the types of
//! an import, a definition to jump to, the files a rename covers) is then
//! given from there, with each file named by its path from the top.

use std::path::{Path, PathBuf};

use super::{Server, uri_to_path};
use crate::project;

/// A walk over a project's folders stops at these: it is run for an editor
/// request, and a document may sit somewhere large.
const MAX_DEPTH: usize = 8;
const MAX_FOLDERS: usize = 200;
const MAX_FILES: usize = 400;
/// How many folders above a document an importer of it is looked for.
const MAX_NAMED_LEVELS: usize = 4;

/// One source file of a project.
pub(super) struct ProjectFile {
    /// Its module name from the top of the project (`Shapes.Geometry`).
    pub module: String,
    /// Its path from the top, with `/` (`shapes/geometry.pyfun`).
    pub rel: String,
    /// Its URI, spelled as the client spells it when the file is open.
    pub uri: String,
    pub path: PathBuf,
}

/// The folder part of a file URI, without the closing slash.
pub(super) fn dir_uri(uri: &str) -> &str {
    match uri.rfind('/') {
        Some(i) => &uri[..i],
        None => "",
    }
}

/// The folder `levels` above the folder URI `dir`.
fn ancestor_uri(dir: &str, levels: usize) -> &str {
    let mut dir = dir;
    for _ in 0..levels {
        dir = dir_uri(dir);
    }
    dir
}

/// The URI of the file at `rel` (a path with `/`) under the folder URI `dir`.
/// Each part is percent-encoded outside the characters a URI leaves alone, so
/// a folder with a space in its name comes out as the client would write it.
pub(super) fn child_uri(dir: &str, rel: &str) -> String {
    let mut uri = String::from(dir);
    for part in rel.split('/') {
        uri.push('/');
        for byte in part.bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                uri.push(byte as char);
            } else {
                uri.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    uri
}

/// Whether a folder's name can be a package's: what `Shapes` in an import
/// becomes, `shapes`.
fn is_package_folder(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Whether a walk goes into the folder `name`: not a hidden one, and not where
/// tools keep what they build or install.
fn is_walked(name: &str) -> bool {
    !(name.starts_with('.')
        || name.starts_with("target")
        || matches!(name, "node_modules" | "__pycache__" | "venv"))
}

/// Every `.pyfun` file under `top`, as paths from it with `/`: a folder's own
/// files in name order, then its folders', nearest the top first, so a walk
/// that runs out of budget has covered what is closest.
pub(super) fn tree_files(top: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let mut folders: Vec<(String, usize)> = vec![(String::new(), 0)];
    let mut visited = 0;
    while visited < folders.len() && visited < MAX_FOLDERS && files.len() < MAX_FILES {
        let (prefix, depth) = folders[visited].clone();
        visited += 1;
        let Ok(entries) = std::fs::read_dir(top.join(&prefix)) else {
            continue;
        };
        let mut names: Vec<(String, bool)> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_str()?.to_string();
                Some((name, entry.file_type().ok()?.is_dir()))
            })
            .collect();
        names.sort();
        for (name, is_dir) in names {
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if is_dir {
                if depth < MAX_DEPTH && is_walked(&name) {
                    folders.push((rel, depth + 1));
                }
            } else if name.ends_with(".pyfun") {
                files.push(rel);
            }
        }
    }
    files
}

impl Server {
    /// The open document at `path`, as its URI.
    fn open_uri(&self, path: &Path) -> Option<&str> {
        self.documents
            .keys()
            .find(|uri| uri_to_path(uri).as_deref() == Some(path))
            .map(String::as_str)
    }

    /// Whether `path` is a module's source: a file on disk, or a document open
    /// in the editor that has not been saved yet.
    fn is_source(&self, path: &Path) -> bool {
        path.is_file() || self.open_uri(path).is_some()
    }

    /// The source at `path`: the open buffer when the file is open, else disk.
    fn text_at(&self, path: &Path) -> Option<String> {
        match self.open_uri(path) {
            Some(uri) => self.documents.get(uri).map(|doc| doc.text.clone()),
            None => std::fs::read_to_string(path).ok(),
        }
    }

    /// How many folders above the document's own the top of its project is.
    fn root_level(&self, uri: &str) -> usize {
        let Some(path) = uri_to_path(uri) else {
            return 0;
        };
        let Some(dir) = path.parent() else {
            return 0;
        };
        let imports = self
            .module_source(uri)
            .map(|source| project::import_names_in(&source))
            .unwrap_or_default();
        let own = project::import_root_level(dir, &imports, |file| self.is_source(file));
        let known = self.roots.borrow().get(uri).copied();
        let named = known.unwrap_or_else(|| {
            let level = self.named_from_above(&path).unwrap_or(0);
            self.roots.borrow_mut().insert(uri.to_string(), level);
            level
        });
        own.max(named)
    }

    /// How far above the document a file imports it by a dotted name, and
    /// means it: `shapes/geometry.pyfun` is one folder below a file that says
    /// `import Shapes.Geometry`. The walk stays inside a repository, and stops
    /// at a folder whose name no import could spell.
    fn named_from_above(&self, path: &Path) -> Option<usize> {
        let mut rel = path.file_name()?.to_str()?.to_string();
        let mut below = path.parent()?;
        for level in 1..=MAX_NAMED_LEVELS {
            if !below.is_dir() || below.join(".git").exists() {
                break;
            }
            let top = below.parent()?;
            // The top of a drive is nobody's project.
            top.parent()?;
            let folder = below.file_name()?.to_str()?;
            if !is_package_folder(folder) {
                break;
            }
            rel = format!("{folder}/{rel}");
            let name = project::module_name_from_rel(&rel)?;
            for other in tree_files(top) {
                if other == rel {
                    continue;
                }
                let other_path = top.join(&other);
                let Some(text) = self.text_at(&other_path) else {
                    continue;
                };
                if text.contains(&name)
                    && project::import_names_in(&text).contains(&name)
                    && means_from(top, &other, &rel)
                {
                    return Some(level);
                }
            }
            below = top;
        }
        None
    }

    /// The top folder of the project `uri` belongs to, as a folder URI.
    pub(super) fn project_dir(&self, uri: &str) -> String {
        ancestor_uri(dir_uri(uri), self.root_level(uri)).to_string()
    }

    /// The name `uri`'s module has in its project: its path from the top
    /// (`Shapes.Geometry` for `shapes/geometry.pyfun`).
    pub(super) fn module_name(&self, uri: &str) -> Option<String> {
        let path = uri_to_path(uri)?;
        let level = self.root_level(uri);
        let mut parts: Vec<&str> = path
            .components()
            .rev()
            .take(level + 1)
            .filter_map(|part| part.as_os_str().to_str())
            .collect();
        parts.reverse();
        project::module_name_from_rel(&parts.join("/"))
    }

    /// The URI of the module `name` in the project `uri` belongs to, and its
    /// source (the open buffer when the file is open, else disk).
    pub(super) fn module_in_project(&self, uri: &str, name: &str) -> Option<(String, String)> {
        let file_uri = child_uri(&self.project_dir(uri), &project::module_file_name(name));
        let source = self.module_source(&file_uri)?;
        Some((file_uri, source))
    }

    /// Every source file of the project `uri` belongs to, the top folder's
    /// first. A document with no file behind it has no project to list.
    pub(super) fn project_files(&self, uri: &str) -> Vec<ProjectFile> {
        let Some(path) = uri_to_path(uri) else {
            return Vec::new();
        };
        let level = self.root_level(uri);
        let Some(top) = path.parent().and_then(|dir| dir.ancestors().nth(level)) else {
            return Vec::new();
        };
        let top_uri = self.project_dir(uri);
        tree_files(top)
            .into_iter()
            .map(|rel| {
                let path = top.join(&rel);
                let uri = match self.open_uri(&path) {
                    Some(open) => open.to_string(),
                    None => child_uri(&top_uri, &rel),
                };
                ProjectFile {
                    module: project::module_name_from_rel(&rel).unwrap_or_default(),
                    rel,
                    uri,
                    path,
                }
            })
            .collect()
    }

    /// Whether the project file at `rel` means the project's own module when
    /// it imports `name`. A file in a folder of its own programs (an
    /// `examples/` beside the project's modules) may have a nearer module of
    /// that name, and then it means that one.
    pub(super) fn imports_from_top(&self, uri: &str, file: &ProjectFile, name: &str) -> bool {
        let Some(path) = uri_to_path(uri) else {
            return false;
        };
        let level = self.root_level(uri);
        match path.parent().and_then(|dir| dir.ancestors().nth(level)) {
            Some(top) => means_from(top, &file.rel, &project::module_file_name(name)),
            None => false,
        }
    }
}

/// Whether the file at `importer` (a path from `top`) reaches the file at
/// `target` when it imports it by that path: no folder between the importer
/// and the top has a file of the same path, which would be the one it means.
fn means_from(top: &Path, importer: &str, target: &str) -> bool {
    let mut folder = match importer.rsplit_once('/') {
        Some((folder, _)) => folder,
        None => return true,
    };
    loop {
        if top.join(folder).join(target).is_file() {
            return false;
        }
        match folder.rsplit_once('/') {
            Some((parent, _)) => folder = parent,
            None => return true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_uri_is_taken_apart_and_put_together() {
        assert_eq!(
            dir_uri("file:///proj/shapes/geometry.pyfun"),
            "file:///proj/shapes"
        );
        assert_eq!(ancestor_uri("file:///proj/shapes", 1), "file:///proj");
        assert_eq!(
            child_uri("file:///proj", "shapes/geometry.pyfun"),
            "file:///proj/shapes/geometry.pyfun"
        );
        assert_eq!(
            child_uri("file:///my%20proj", "more work/a.pyfun"),
            "file:///my%20proj/more%20work/a.pyfun"
        );
    }

    #[test]
    fn a_package_folder_is_named_as_an_import_spells_it() {
        assert!(is_package_folder("shapes"));
        assert!(is_package_folder("train_times"));
        assert!(!is_package_folder("Shapes"));
        assert!(!is_package_folder("my-project"));
        assert!(!is_package_folder("2d"));
    }
}
