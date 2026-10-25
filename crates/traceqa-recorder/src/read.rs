//! Reads a run folder back. Commands that work on finished runs (diff, findings, codegen,
//! heal, verify, runs) all find their input through [`open_run`], so none of them
//! re-implements run discovery.
//!
//! Reads never follow a symbolic link and are capped at [`MAX_FILE_BYTES`]. The MCP crate keeps
//! its own, stricter reader: it also has to keep every path inside one runs folder.

use std::fmt;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::de::DeserializeOwned;
use traceqa_common::types::{AxeResults, ComponentsFile, RunManifest, TargetSummary};

/// The largest `run.json` or `components.json` that is read.
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Why a run folder could not be read. The text names the path and says what to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadError(String);

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ReadError {}

fn fail<T>(message: String) -> Result<T, ReadError> {
    Err(ReadError(message))
}

/// One browser-and-viewport folder inside a run.
#[derive(Debug, Clone)]
pub struct TargetRef {
    /// The folder's name inside the run, such as `chromium-1280x800`.
    pub dir: String,
    /// The folder itself.
    pub path: PathBuf,
    /// What `run.json` says about it, when `run.json` was found.
    pub summary: Option<TargetSummary>,
}

/// What [`open_run`] found.
#[derive(Debug, Clone)]
pub struct RunFolder {
    /// The run folder, or the target folder when no `run.json` sits above it.
    pub path: PathBuf,
    pub manifest: Option<RunManifest>,
    /// Every target of the run, or only the one that was named.
    pub targets: Vec<TargetRef>,
}

/// Finds a run from what a person typed: a run folder, a target folder, a `components.json`
/// path, or a bare run id that is a folder inside `runs_dir`. A path that exists wins over an id.
pub fn open_run(arg: &str, runs_dir: &Path) -> Result<RunFolder, ReadError> {
    let given = Path::new(arg);
    if given.exists() {
        return open_path(given);
    }
    let is_bare_id = !arg.is_empty() && !arg.contains(['/', '\\']) && arg != "." && arg != "..";
    if is_bare_id {
        let dir = runs_dir.join(arg);
        if dir.is_dir() {
            if !dir.join("run.json").exists() {
                return fail(format!("run {arg} has no run.json in {}", dir.display()));
            }
            return open_run_dir(&dir);
        }
        return fail(format!("no run {arg} in {}", runs_dir.display()));
    }
    fail(format!("{arg} does not exist"))
}

fn open_path(path: &Path) -> Result<RunFolder, ReadError> {
    if path.is_file() {
        if path.file_name().is_some_and(|name| name == "components.json") {
            let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
            return open_target_dir(dir.unwrap_or(Path::new(".")));
        }
        return fail(format!(
            "{} is a file; give a run folder, a target folder or a components.json",
            path.display()
        ));
    }
    if path.join("run.json").exists() {
        return open_run_dir(path);
    }
    if path.join("components.json").exists() {
        return open_target_dir(path);
    }
    fail(format!(
        "{} holds neither run.json nor components.json; is it a TraceQA run folder?",
        path.display()
    ))
}

fn open_run_dir(dir: &Path) -> Result<RunFolder, ReadError> {
    let manifest: RunManifest = read_json(&dir.join("run.json"))?;
    let mut targets = Vec::new();
    for summary in &manifest.targets {
        targets.push(target_ref(dir, summary.clone())?);
    }
    Ok(RunFolder {
        path: dir.to_path_buf(),
        manifest: Some(manifest),
        targets,
    })
}

/// A target folder: the run above it, if it has a `run.json`, supplies the manifest.
fn open_target_dir(dir: &Path) -> Result<RunFolder, ReadError> {
    let name = absolute_name(dir);
    let parent = dir.parent().filter(|p| !p.as_os_str().is_empty());
    let run_dir = parent.filter(|p| p.join("run.json").is_file());
    let manifest = match run_dir {
        Some(run_dir) => Some(read_json::<RunManifest>(&run_dir.join("run.json"))?),
        None => None,
    };
    let summary = manifest.as_ref().and_then(|m| {
        m.targets
            .iter()
            .find(|t| Some(t.dir.as_str()) == name.as_deref())
            .cloned()
    });
    let target = TargetRef {
        dir: name.unwrap_or_default(),
        path: dir.to_path_buf(),
        summary,
    };
    Ok(RunFolder {
        path: run_dir.unwrap_or(dir).to_path_buf(),
        manifest,
        targets: vec![target],
    })
}

fn absolute_name(dir: &Path) -> Option<String> {
    let absolute = std::path::absolute(dir).ok()?;
    absolute.file_name().map(|n| n.to_string_lossy().into_owned())
}

fn target_ref(run_dir: &Path, summary: TargetSummary) -> Result<TargetRef, ReadError> {
    let mut parts = Path::new(&summary.dir).components();
    if !matches!((parts.next(), parts.next()), (Some(Component::Normal(_)), None)) {
        return fail(format!(
            "{} names a target folder {:?} that is not a plain folder name",
            run_dir.join("run.json").display(),
            summary.dir
        ));
    }
    Ok(TargetRef {
        dir: summary.dir.clone(),
        path: run_dir.join(&summary.dir),
        summary: Some(summary),
    })
}

/// Reads and parses a target's `components.json`.
pub fn read_components(target: &TargetRef) -> Result<ComponentsFile, ReadError> {
    read_json(&target.path.join("components.json"))
}

/// Reads a target's `axe.json`. `None` when the run did not ask for axe-core, which is not an error.
pub fn read_axe(target: &TargetRef) -> Result<Option<AxeResults>, ReadError> {
    let path = target.path.join("axe.json");
    if fs::symlink_metadata(&path).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) {
        return Ok(None);
    }
    read_json(&path).map(Some)
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, ReadError> {
    let meta = fs::symlink_metadata(path).map_err(|e| ReadError(format!("could not read {}: {e}", path.display())))?;
    if meta.file_type().is_symlink() {
        return fail(format!("{} is a symbolic link; refusing to follow it", path.display()));
    }
    if !meta.is_file() {
        return fail(format!("{} is not a file", path.display()));
    }
    let mib = MAX_FILE_BYTES / (1024 * 1024);
    if meta.len() > MAX_FILE_BYTES {
        return fail(format!(
            "{} is too large to read ({} bytes; the limit is {mib} MiB)",
            path.display(),
            meta.len(),
        ));
    }
    // The size may change between the check and the read, so the read is capped too.
    let mut text = String::new();
    File::open(path)
        .and_then(|file| file.take(MAX_FILE_BYTES + 1).read_to_string(&mut text))
        .map_err(|e| ReadError(format!("could not read {}: {e}", path.display())))?;
    if text.len() as u64 > MAX_FILE_BYTES {
        return fail(format!("{} grew past the {mib} MiB limit", path.display()));
    }
    serde_json::from_str(&text).map_err(|e| ReadError(format!("{} is not valid: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::new_run_id;

    const MANIFEST: &str = include_str!("../../../spec/examples/RunManifest.completed.json");
    const COMPONENTS: &str = include_str!("../../../spec/examples/ComponentsFile.shop.json");

    /// A run folder in a fresh temporary folder, laid out as the manifest example describes.
    struct Fixture {
        root: PathBuf,
        run: PathBuf,
        target: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("traceqa-read-{}", new_run_id()));
            let manifest: RunManifest = serde_json::from_str(MANIFEST).unwrap();
            let run = root.join(&manifest.run_id);
            let target = run.join(&manifest.targets[0].dir);
            fs::create_dir_all(&target).unwrap();
            fs::write(run.join("run.json"), MANIFEST).unwrap();
            fs::write(target.join("components.json"), COMPONENTS).unwrap();
            Self { root, run, target }
        }

        fn id(&self) -> String {
            self.run.file_name().unwrap().to_str().unwrap().to_string()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn text(path: &Path) -> String {
        path.to_str().unwrap().to_string()
    }

    #[test]
    fn a_run_folder_lists_its_targets() {
        let f = Fixture::new();
        let found = open_run(&text(&f.run), &f.root).unwrap();
        assert_eq!(found.manifest.unwrap().run_id, f.id());
        assert_eq!(found.targets.len(), 1);
        assert_eq!(found.targets[0].dir, "firefox-1280x800");
        assert!(found.targets[0].summary.is_some());
        assert!(!read_components(&found.targets[0]).unwrap().components.is_empty());
    }

    #[test]
    fn a_target_folder_finds_the_run_above_it() {
        let f = Fixture::new();
        let found = open_run(&text(&f.target), &f.root).unwrap();
        assert!(found.manifest.is_some());
        assert_eq!(found.targets.len(), 1);
        assert_eq!(found.targets[0].dir, "firefox-1280x800");
        assert!(found.targets[0].summary.is_some());
    }

    #[test]
    fn a_components_json_path_names_its_target() {
        let f = Fixture::new();
        let found = open_run(&text(&f.target.join("components.json")), &f.root).unwrap();
        assert_eq!(found.targets[0].path, f.target);
        assert!(found.manifest.is_some());
    }

    #[test]
    fn a_target_folder_without_a_run_has_no_manifest() {
        let f = Fixture::new();
        fs::remove_file(f.run.join("run.json")).unwrap();
        let found = open_run(&text(&f.target), &f.root).unwrap();
        assert!(found.manifest.is_none() && found.targets[0].summary.is_none());
        assert!(read_components(&found.targets[0]).is_ok());
    }

    #[test]
    fn a_bare_id_is_looked_up_in_the_runs_folder() {
        let f = Fixture::new();
        let found = open_run(&f.id(), &f.root).unwrap();
        assert_eq!(found.path, f.run);
        let error = open_run("r_nope", &f.root).unwrap_err().to_string();
        assert!(error.contains("no run r_nope"), "{error}");
    }

    #[test]
    fn a_missing_run_json_is_named() {
        let f = Fixture::new();
        fs::remove_file(f.run.join("run.json")).unwrap();
        fs::remove_file(f.target.join("components.json")).unwrap();
        let error = open_run(&f.id(), &f.root).unwrap_err().to_string();
        assert!(error.contains("has no run.json"), "{error}");
        let error = open_run(&text(&f.run), &f.root).unwrap_err().to_string();
        assert!(error.contains("neither run.json nor components.json"), "{error}");
    }

    #[test]
    fn a_target_name_that_leaves_the_run_is_refused() {
        let f = Fixture::new();
        fs::write(
            f.run.join("run.json"),
            MANIFEST.replace("firefox-1280x800", "../escape"),
        )
        .unwrap();
        let error = open_run(&text(&f.run), &f.root).unwrap_err().to_string();
        assert!(error.contains("not a plain folder name"), "{error}");
    }

    #[test]
    fn a_symlinked_components_json_is_refused() {
        let f = Fixture::new();
        let real = f.root.join("elsewhere.json");
        fs::write(&real, COMPONENTS).unwrap();
        let link = f.target.join("components.json");
        fs::remove_file(&link).unwrap();
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&real, &link);
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(&real, &link);
        if made.is_err() {
            eprintln!("skipped: this account cannot create symbolic links");
            return;
        }
        let found = open_run(&text(&f.run), &f.root).unwrap();
        let error = read_components(&found.targets[0]).unwrap_err().to_string();
        assert!(error.contains("symbolic link"), "{error}");
    }

    #[test]
    fn a_file_over_the_cap_is_refused_with_its_size() {
        let f = Fixture::new();
        let file = File::create(f.target.join("components.json")).unwrap();
        file.set_len(MAX_FILE_BYTES + 1).unwrap(); // sparse where the file system allows it
        drop(file);
        let found = open_run(&text(&f.run), &f.root).unwrap();
        let error = read_components(&found.targets[0]).unwrap_err().to_string();
        assert!(error.contains("too large") && error.contains("64 MiB"), "{error}");
    }

    #[test]
    fn broken_json_names_the_file() {
        let f = Fixture::new();
        fs::write(f.target.join("components.json"), "{").unwrap();
        let found = open_run(&text(&f.run), &f.root).unwrap();
        let error = read_components(&found.targets[0]).unwrap_err().to_string();
        assert!(error.contains("components.json is not valid"), "{error}");
    }
}
