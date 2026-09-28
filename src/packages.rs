//! `pyfun add` and `pyfun install`: façade dependencies (`DESIGN.md` §6.2).
//!
//! A façade is an ordinary pip distribution that ships typed `extern`
//! declarations as `.pyfun` files. `add` installs it with the environment's own
//! installer (`uv pip` when `uv` is on PATH, else `python -m pip`), copies its
//! `.pyfun` files into `<root>/.pyfun/facades/<dist>/`, and records the installed
//! version in `pyfun.toml`; `install` does the same for every dependency the
//! manifest lists, pinned to its recorded version. The compiler reads only the
//! vendored copies, so checking and compiling stay offline.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use pyfun::project::manifest;

/// `pyfun add <requirement>...`
pub fn add(specs: &[String]) -> ExitCode {
    if specs.is_empty() {
        return fail("`add` needs a package, e.g. `pyfun add pyfun-textwrap`");
    }
    let root = match project_root(true) {
        Ok(root) => root,
        Err(msg) => return fail(&msg),
    };
    for spec in specs {
        let dist = match dist_name(spec) {
            Ok(dist) => dist,
            Err(msg) => return fail(&msg),
        };
        if let Err(msg) = install_and_vendor(&root, spec, &dist) {
            return fail(&msg);
        }
    }
    ExitCode::SUCCESS
}

/// `pyfun install`: every dependency in `pyfun.toml`, at its recorded version.
pub fn install() -> ExitCode {
    let root = match project_root(false) {
        Ok(root) => root,
        Err(msg) => return fail(&msg),
    };
    let text = std::fs::read_to_string(root.join(manifest::MANIFEST)).unwrap_or_default();
    let deps = manifest::table(&text, "dependencies");
    if deps.is_empty() {
        println!("no dependencies in {}", manifest::MANIFEST);
        return ExitCode::SUCCESS;
    }
    for (dist, version) in deps {
        let spec = if version.is_empty() {
            dist.clone()
        } else {
            format!("{dist}=={version}")
        };
        if let Err(msg) = install_and_vendor(&root, &spec, &dist) {
            return fail(&msg);
        }
    }
    ExitCode::SUCCESS
}

/// The project root: the nearest `pyfun.toml` at or above the working
/// directory. `add` creates one in the working directory when there is none.
fn project_root(create: bool) -> Result<PathBuf, String> {
    let cwd =
        std::env::current_dir().map_err(|e| format!("cannot read the working directory: {e}"))?;
    if let Some(root) = manifest::find_root(&cwd) {
        return Ok(root);
    }
    if !create {
        return Err(format!(
            "no {} here or in any parent directory (`pyfun add <package>` creates one)",
            manifest::MANIFEST
        ));
    }
    let name = cwd
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".to_string());
    std::fs::write(cwd.join(manifest::MANIFEST), manifest::new_manifest(&name))
        .map_err(|e| format!("cannot write {}: {e}", manifest::MANIFEST))?;
    println!("created {}", manifest::MANIFEST);
    Ok(cwd)
}

/// The distribution a requirement names. A name with an optional version
/// specifier is read directly; a local directory is read from its
/// `pyproject.toml`'s `[project] name`.
fn dist_name(spec: &str) -> Result<String, String> {
    let path = Path::new(spec);
    if path.is_dir() {
        let text = std::fs::read_to_string(path.join("pyproject.toml"))
            .map_err(|_| format!("`{spec}` is a directory with no pyproject.toml"))?;
        return manifest::table(&text, "project")
            .into_iter()
            .find(|(k, _)| k == "name")
            .map(|(_, v)| v)
            .ok_or_else(|| format!("`{spec}/pyproject.toml` has no [project] name"));
    }
    let end = spec
        .find(['=', '<', '>', '!', '~', ';', '[', ' ', '@'])
        .unwrap_or(spec.len());
    let name = spec[..end].trim();
    if name.is_empty() {
        return Err(format!("cannot read a package name from `{spec}`"));
    }
    Ok(name.to_string())
}

fn install_and_vendor(root: &Path, spec: &str, dist: &str) -> Result<(), String> {
    let python = crate::python_cmd().ok_or("no Python interpreter found (set PYFUN_PYTHON)")?;
    run_installer(&python, spec)?;
    let (version, files) = facade_files(&python, dist)?;
    if files.is_empty() {
        return Err(format!(
            "`{dist}` installed, but it ships no .pyfun files, so it is not a Pyfun façade"
        ));
    }
    let target = root
        .join(manifest::FACADES)
        .join(manifest::normalize_dist(dist));
    let _ = std::fs::remove_dir_all(&target);
    std::fs::create_dir_all(&target)
        .map_err(|e| format!("cannot create {}: {e}", target.display()))?;
    let mut modules = Vec::new();
    for file in &files {
        let name = file
            .file_name()
            .ok_or_else(|| format!("odd façade path {}", file.display()))?;
        std::fs::copy(file, target.join(name))
            .map_err(|e| format!("cannot copy {}: {e}", file.display()))?;
        if let Some(module) = pyfun::project::module_name_from_path(Path::new(name)) {
            modules.push(module);
        }
    }
    let manifest_path = root.join(manifest::MANIFEST);
    let text = std::fs::read_to_string(&manifest_path).unwrap_or_default();
    let text = manifest::set(&text, "dependencies", dist, &version);
    std::fs::write(&manifest_path, text)
        .map_err(|e| format!("cannot write {}: {e}", manifest_path.display()))?;
    modules.sort();
    println!("added {dist} {version} (import {})", modules.join(", "));
    Ok(())
}

/// Install `spec` into the interpreter's environment: `uv pip` when `uv` is on
/// PATH (pointed at that interpreter), else `python -m pip`.
fn run_installer(python: &str, spec: &str) -> Result<(), String> {
    let uv = Command::new("uv").arg("--version").output().is_ok();
    let status = if uv {
        Command::new("uv")
            .args(["pip", "install", "--python", python, spec])
            .status()
    } else {
        Command::new(python)
            .args(["-m", "pip", "install", spec])
            .status()
    };
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(_) => Err(format!(
            "installing `{spec}` failed (see the installer's output above)"
        )),
        Err(e) => Err(format!("cannot run the installer: {e}")),
    }
}

/// The installed version of `dist` and the paths of the `.pyfun` files it
/// ships, read from its installed metadata.
fn facade_files(python: &str, dist: &str) -> Result<(String, Vec<PathBuf>), String> {
    const SCRIPT: &str = "import importlib.metadata as m, sys\n\
                          d = m.distribution(sys.argv[1])\n\
                          print(d.version)\n\
                          for f in d.files or []:\n    \
                              if str(f).endswith('.pyfun'):\n        \
                                  print(d.locate_file(f))\n";
    let out = Command::new(python)
        .args(["-c", SCRIPT, dist])
        .output()
        .map_err(|e| format!("cannot run {python}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cannot read the installed metadata of `{dist}`: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    let version = lines.next().unwrap_or_default().trim().to_string();
    let files = lines.map(|l| PathBuf::from(l.trim())).collect();
    Ok((version, files))
}

fn fail(msg: &str) -> ExitCode {
    eprintln!("error: {msg}");
    ExitCode::FAILURE
}
