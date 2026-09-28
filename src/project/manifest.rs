//! The project manifest, `pyfun.toml` (`DESIGN.md` §6.2), and the vendored
//! façades it describes.
//!
//! A manifest marks a project root and lists its **façade** dependencies: pip
//! distributions that ship typed `extern` declarations as `.pyfun` files.
//! `pyfun add` installs one, copies its `.pyfun` files into
//! `<root>/.pyfun/facades/<dist>/`, and records the installed version here;
//! `import Name` then resolves a module from a sibling file first and from the
//! vendored façades second ([`super::locate_module`]). Compiling needs neither
//! Python nor the network, since everything it reads is inside the project.
//!
//! The crate takes no dependencies, so this reads and edits only the small
//! subset of TOML a manifest uses: `[table]` headers and `key = "string"`
//! lines. Edits are line-level, so comments and layout survive.

use std::path::{Path, PathBuf};

/// The manifest's file name.
pub const MANIFEST: &str = "pyfun.toml";

/// Where vendored façades live, relative to the project root.
pub const FACADES: &str = ".pyfun/facades";

/// The nearest directory at or above `start` that holds a `pyfun.toml`.
pub fn find_root(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        if d.join(MANIFEST).is_file() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

/// Each vendored façade's directory under `root`, in name order.
pub fn facade_dirs(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root.join(FACADES)) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs
}

/// A new manifest for a project called `name`.
pub fn new_manifest(name: &str) -> String {
    format!("[project]\nname = \"{name}\"\n\n[dependencies]\n")
}

/// The `key = "value"` entries of `[table]`, in file order.
pub fn table(text: &str, table: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        if let Some(header) = header_of(line) {
            inside = header == table;
            continue;
        }
        if inside && let Some((key, value)) = entry_of(line) {
            out.push((key, value));
        }
    }
    out
}

/// `text` with `[table]`'s `key` set to `value`: an existing entry is replaced in
/// place, a new one goes after the table's last entry, and a missing table is
/// appended. Every other line is kept as it was.
pub fn set(text: &str, table: &str, key: &str, value: &str) -> String {
    let entry = format!("{key} = \"{}\"", escape(value));
    let lines: Vec<&str> = text.lines().collect();
    let mut inside = false;
    let mut table_end = None; // index after the table's last entry
    for (i, raw) in lines.iter().enumerate() {
        let line = raw.trim();
        if let Some(header) = header_of(line) {
            inside = header == table;
            if inside {
                table_end = Some(i + 1);
            }
            continue;
        }
        if inside {
            if let Some((k, _)) = entry_of(line)
                && k == key
            {
                let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
                out[i] = entry;
                return join(out);
            }
            if !line.is_empty() && !line.starts_with('#') {
                table_end = Some(i + 1);
            }
        }
    }
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    match table_end {
        Some(at) => out.insert(at, entry),
        None => {
            if out.last().is_some_and(|l| !l.trim().is_empty()) {
                out.push(String::new());
            }
            out.push(format!("[{table}]"));
            out.push(entry);
        }
    }
    join(out)
}

/// The normalized name of a distribution (PEP 503): lowercase, runs of
/// `-`, `_` and `.` folded to `-`. Used for the façade's directory.
pub fn normalize_dist(name: &str) -> String {
    let mut out = String::new();
    let mut sep = false;
    for c in name.chars() {
        if matches!(c, '-' | '_' | '.') {
            sep = true;
        } else {
            if sep && !out.is_empty() {
                out.push('-');
            }
            sep = false;
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

fn header_of(line: &str) -> Option<&str> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    Some(inner.trim())
}

fn entry_of(line: &str) -> Option<(String, String)> {
    if line.starts_with('#') {
        return None;
    }
    let (key, value) = line.split_once('=')?;
    let key = key.trim().trim_matches('"').to_string();
    let value = value.trim();
    let value = value.split_once(" #").map_or(value, |(v, _)| v).trim();
    let unquoted = value.strip_prefix('"')?.strip_suffix('"')?;
    Some((key, unquoted.replace("\\\"", "\"").replace("\\\\", "\\")))
}

fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn join(lines: Vec<String>) -> String {
    let mut s = lines.join("\n");
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_table() {
        let text = "[project]\nname = \"demo\"\n\n[dependencies]\n# the façades\npyfun-textwrap = \"0.1.0\"\n";
        assert_eq!(
            table(text, "dependencies"),
            vec![("pyfun-textwrap".to_string(), "0.1.0".to_string())]
        );
        assert_eq!(table(text, "project")[0].1, "demo");
    }

    #[test]
    fn set_replaces_appends_and_creates() {
        let text = new_manifest("demo");
        let text = set(&text, "dependencies", "a", "1.0");
        let text = set(&text, "dependencies", "b", "2.0");
        let text = set(&text, "dependencies", "a", "1.1");
        assert_eq!(
            table(&text, "dependencies"),
            vec![
                ("a".to_string(), "1.1".to_string()),
                ("b".to_string(), "2.0".to_string())
            ]
        );
        let bare = set("[project]\nname = \"x\"\n", "dependencies", "c", "3");
        assert_eq!(table(&bare, "dependencies")[0].0, "c");
        // Comments and other tables survive an edit.
        let commented = set("# top\n[project]\nname = \"x\"\n", "project", "name", "y");
        assert!(commented.starts_with("# top\n"));
        assert_eq!(table(&commented, "project")[0].1, "y");
    }

    #[test]
    fn normalizes_distribution_names() {
        assert_eq!(normalize_dist("Pyfun_Text.Wrap"), "pyfun-text-wrap");
        assert_eq!(normalize_dist("pyfun--x"), "pyfun-x");
    }

    #[test]
    fn finds_the_root_and_its_facades() {
        let root = std::env::temp_dir().join(format!("pyfun_manifest_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::create_dir_all(root.join(FACADES).join("pyfun-b")).unwrap();
        std::fs::create_dir_all(root.join(FACADES).join("pyfun-a")).unwrap();
        std::fs::write(root.join(MANIFEST), new_manifest("demo")).unwrap();
        assert_eq!(
            find_root(&root.join("src/deep")).as_deref(),
            Some(root.as_path())
        );
        let names: Vec<_> = facade_dirs(&root)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, ["pyfun-a", "pyfun-b"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
