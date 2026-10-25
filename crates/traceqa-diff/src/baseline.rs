//! Baselines: a saved copy of a run's `components.json` files that a later run is compared with.
//!
//! [`BaselineStore`] is the only code that writes a baseline folder (`spec/baseline.schema.json`).
//! It never updates one silently: [`BaselineStore::save`] refuses to replace, and
//! [`BaselineStore::promote`] replaces only when the caller says so (the command line asks for
//! `--yes`). Writes go to a staging folder next to the baseline and are renamed into place, so a
//! failed write leaves the old baseline whole.
//!
//! Layout: `<root>/<name>/baseline.json`, `<root>/<name>/<target dir>/components.json` and, when the
//! saved run had them, `<root>/<name>/<target dir>/crops/*.png`. Nothing else is copied: no
//! `run.json` (it holds error text), no storage state, no events.
//!
//! Files in a baseline folder may come from a pull request, so reads refuse symbolic links and are
//! capped in size.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use traceqa_common::types::{
    BaselineAlias, BaselineFile, BaselinePromotion, BaselineSource, BaselineTarget, BrowserInfo, ComponentsFile,
    DiffFile, PairLink, Viewport,
};

use crate::gate::ALIASED;

/// `BaselineFile.formatVersion`.
pub const FORMAT_VERSION: u64 = 1;
/// How many `promotions` entries are kept.
pub const MAX_PROMOTIONS: usize = 20;
/// The largest file read from a baseline folder.
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// The most crops copied per target; a page has far fewer components than this.
const MAX_CROPS: usize = 20_000;

/// Why a baseline could not be read or written. The text says what to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaselineError {
    /// `save` found a baseline of that name.
    Exists {
        name: String,
        path: PathBuf,
    },
    /// No baseline of that name; `known` lists the ones in the folder.
    Missing {
        name: String,
        root: PathBuf,
        known: Vec<String>,
    },
    BadName(String),
    Invalid(String),
    Io(String),
}

impl fmt::Display for BaselineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exists { name, path } => write!(
                f,
                "baseline {name} already exists at {}; it is never replaced silently: use `traceqa baseline \
                 promote` (with --yes) to replace it",
                path.display()
            ),
            Self::Missing { name, root, known } => {
                write!(f, "no baseline {name} in {}", root.display())?;
                if known.is_empty() {
                    write!(f, "; make one with `traceqa baseline save RUN`")
                } else {
                    write!(f, "; there is: {}", known.join(", "))
                }
            }
            Self::BadName(why) | Self::Invalid(why) | Self::Io(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for BaselineError {}

fn io_error(what: &str, path: &Path, e: &std::io::Error) -> BaselineError {
    BaselineError::Io(format!("{what} {}: {e}", path.display()))
}

/// One target folder of the run to save.
#[derive(Debug, Clone)]
pub struct TargetSource {
    /// The folder's name inside the run, such as `chromium-1280x800`.
    pub dir: String,
    /// The folder itself.
    pub path: PathBuf,
    pub browser: BrowserInfo,
    pub viewport: Viewport,
}

/// What `save` and `promote` copy, and what they record about it.
#[derive(Debug, Clone)]
pub struct SaveInput {
    pub run_id: String,
    pub engine_version: String,
    /// The page address as `components.json` has it (secrets already replaced).
    pub url: String,
    pub fingerprint_version: u64,
    /// RFC 3339 UTC, for `promotedAt`.
    pub now: String,
    /// What this changes against the baseline it replaces, for `promotions`.
    pub summary: String,
    pub targets: Vec<TargetSource>,
}

/// A new rename link.
#[derive(Debug, Clone)]
pub struct AliasInput {
    pub from: String,
    pub to: String,
    pub reason: String,
    pub evidence: Option<String>,
    pub now: String,
}

/// A baseline read from disk.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub file: BaselineFile,
    pub dir: PathBuf,
}

impl Loaded {
    /// The saved folder of a target.
    pub fn target_dir(&self, dir: &str) -> PathBuf {
        self.dir.join(dir)
    }

    /// Reads a saved target's `components.json`.
    pub fn read_components(&self, dir: &str) -> Result<ComponentsFile, BaselineError> {
        read_json(&self.target_dir(dir).join("components.json"))
    }
}

/// The result of a promote.
#[derive(Debug, Clone)]
pub struct Promoted {
    pub file: BaselineFile,
    /// Aliases dropped because their `from` id is not in the new baseline.
    pub pruned_aliases: usize,
}

/// A folder of baselines.
#[derive(Debug, Clone)]
pub struct BaselineStore {
    root: PathBuf,
}

/// The default name of the baseline of a page: its host made safe for file names, a dash and 8 hex
/// characters of the SHA-256 of the address. The address is the redacted one from `components.json`,
/// so the key never depends on a secret.
pub fn key_for(url: &str) -> String {
    let host = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_default();
    let mut safe: String = host
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    safe = safe.trim_matches(|c| c == '.' || c == '-').chars().take(40).collect();
    let safe = safe.trim_matches(|c| c == '.' || c == '-');
    let safe = if safe.is_empty() { "page" } else { safe };
    let digest = Sha256::digest(url.as_bytes());
    let hex: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
    format!("{safe}-{hex}")
}

/// Checks a baseline name: 1 to 64 of letters, digits, `.`, `_` and `-`, not starting with a dot,
/// and not a name Windows reserves. It becomes a folder name, so it cannot hold a separator.
pub fn check_name(name: &str) -> Result<(), BaselineError> {
    let ok_chars = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.ends_with(|c: char| c.is_ascii_digit()));
    if name.is_empty() || name.len() > 64 || !ok_chars || name.starts_with('.') || reserved {
        return Err(BaselineError::BadName(format!(
            "baseline name {name:?} is not usable: use 1 to 64 letters, digits, '.', '_' or '-', not starting with \
             a dot"
        )));
    }
    Ok(())
}

fn check_target_dir(dir: &str) -> Result<(), BaselineError> {
    if dir.is_empty() || dir == "." || dir == ".." || dir.contains(['/', '\\', ':']) || dir.starts_with('.') {
        return Err(BaselineError::Invalid(format!(
            "target folder {dir:?} is not a plain folder name"
        )));
    }
    Ok(())
}

impl BaselineStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The folder of a baseline of this name (which may not exist).
    pub fn dir_of(&self, name: &str) -> Result<PathBuf, BaselineError> {
        check_name(name)?;
        Ok(self.root.join(name))
    }

    /// The names of the baselines in the folder, sorted.
    pub fn list(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().join("baseline.json").is_file())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| !n.starts_with('.'))
            .collect();
        names.sort();
        names
    }

    /// Reads the baseline called `name`.
    pub fn load(&self, name: &str) -> Result<Loaded, BaselineError> {
        let dir = self.dir_of(name)?;
        if !dir.join("baseline.json").is_file() {
            return Err(BaselineError::Missing {
                name: name.to_string(),
                root: self.root.clone(),
                known: self.list(),
            });
        }
        Self::load_dir(&dir)
    }

    /// Reads the baseline in a folder, wherever it is.
    pub fn load_dir(dir: &Path) -> Result<Loaded, BaselineError> {
        let file: BaselineFile = read_json(&dir.join("baseline.json"))?;
        if file.format_version > FORMAT_VERSION {
            return Err(BaselineError::Invalid(format!(
                "{} has format version {}, newer than this TraceQA understands ({FORMAT_VERSION}); update TraceQA",
                dir.join("baseline.json").display(),
                file.format_version
            )));
        }
        for target in &file.targets {
            check_target_dir(&target.dir)?;
        }
        Ok(Loaded {
            file,
            dir: dir.to_path_buf(),
        })
    }

    /// Makes a new baseline. Fails when the name is taken. `name` defaults to [`key_for`] the address.
    pub fn save(&self, name: Option<&str>, input: &SaveInput) -> Result<BaselineFile, BaselineError> {
        let name = name.map_or_else(|| key_for(&input.url), str::to_string);
        let dir = self.dir_of(&name)?;
        if dir.exists() {
            return Err(BaselineError::Exists { name, path: dir });
        }
        let stage = self.stage(&name)?;
        let result = (|| {
            let (file, _) = self.write_stage(&stage, &name, input, Vec::new(), Vec::new())?;
            fs::rename(&stage, &dir).map_err(|e| io_error("could not move the new baseline to", &dir, &e))?;
            Ok(file)
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&stage);
        }
        result
    }

    /// Replaces the baseline called `name` with a run. Its aliases whose `from` id is gone from the
    /// new baseline are dropped, because they can no longer apply; its history is kept.
    pub fn promote(&self, name: &str, input: &SaveInput) -> Result<Promoted, BaselineError> {
        let old = self.load(name)?;
        let dir = self.dir_of(name)?;
        let stage = self.stage(name)?;
        let result = (|| {
            let (file, pruned) = self.write_stage(
                &stage,
                name,
                input,
                old.file.aliases.clone(),
                old.file.promotions.clone(),
            )?;
            let backup = self.root.join(format!(".{name}.old-{}", std::process::id()));
            let _ = fs::remove_dir_all(&backup);
            fs::rename(&dir, &backup).map_err(|e| io_error("could not move aside", &dir, &e))?;
            if let Err(e) = fs::rename(&stage, &dir) {
                let _ = fs::rename(&backup, &dir);
                return Err(io_error("could not move the new baseline to", &dir, &e));
            }
            let _ = fs::remove_dir_all(&backup);
            Ok(Promoted {
                file,
                pruned_aliases: pruned,
            })
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&stage);
        }
        result
    }

    /// Records that baseline component `from` is run component `to`. An older alias from the same
    /// id is replaced. `from` must be in the baseline.
    pub fn add_alias(&self, name: &str, alias: &AliasInput) -> Result<BaselineFile, BaselineError> {
        check_component_id(&alias.from)?;
        check_component_id(&alias.to)?;
        if alias.from == alias.to {
            return Err(BaselineError::Invalid("an alias needs two different ids".into()));
        }
        let loaded = self.load(name)?;
        let mut known = false;
        for target in &loaded.file.targets {
            known |= loaded
                .read_components(&target.dir)?
                .components
                .iter()
                .any(|c| c.id == alias.from);
        }
        if !known {
            return Err(BaselineError::Invalid(format!(
                "{} is not a component of baseline {name}; an alias starts from a component that the baseline has",
                alias.from
            )));
        }
        let mut file = loaded.file;
        file.aliases.retain(|a| a.from != alias.from);
        file.aliases.push(BaselineAlias {
            from: alias.from.clone(),
            to: alias.to.clone(),
            reason: alias.reason.clone(),
            evidence: alias.evidence.clone(),
            added_at: alias.now.clone(),
        });
        write_baseline_json(&loaded.dir, &file)?;
        Ok(file)
    }

    fn stage(&self, name: &str) -> Result<PathBuf, BaselineError> {
        fs::create_dir_all(&self.root).map_err(|e| io_error("could not create", &self.root, &e))?;
        let stage = self.root.join(format!(".{name}.new-{}", std::process::id()));
        let _ = fs::remove_dir_all(&stage);
        fs::create_dir(&stage).map_err(|e| io_error("could not create", &stage, &e))?;
        Ok(stage)
    }

    /// Copies the targets into `stage` and writes its `baseline.json`.
    fn write_stage(
        &self,
        stage: &Path,
        name: &str,
        input: &SaveInput,
        aliases: Vec<BaselineAlias>,
        mut promotions: Vec<BaselinePromotion>,
    ) -> Result<(BaselineFile, usize), BaselineError> {
        if input.targets.is_empty() {
            return Err(BaselineError::Invalid("the run has no finished target to save".into()));
        }
        let mut targets = Vec::new();
        let mut ids = BTreeSet::new();
        let mut seen = BTreeSet::new();
        for source in &input.targets {
            check_target_dir(&source.dir)?;
            if !seen.insert(source.dir.clone()) {
                return Err(BaselineError::Invalid(format!("target {} is listed twice", source.dir)));
            }
            let to = stage.join(&source.dir);
            fs::create_dir(&to).map_err(|e| io_error("could not create", &to, &e))?;
            let components: ComponentsFile = read_json(&source.path.join("components.json"))?;
            if components.fingerprint_version != input.fingerprint_version {
                return Err(BaselineError::Invalid(format!(
                    "target {} has fingerprint version {}, but the run has {}; the targets of one baseline must \
                     agree",
                    source.dir, components.fingerprint_version, input.fingerprint_version
                )));
            }
            ids.extend(components.components.iter().map(|c| c.id.clone()));
            copy_file(&source.path.join("components.json"), &to.join("components.json"))?;
            let has_crops = copy_crops(&source.path.join("crops"), &to.join("crops"))?;
            targets.push(BaselineTarget {
                dir: source.dir.clone(),
                browser: source.browser.clone(),
                viewport: source.viewport.clone(),
                has_crops,
            });
        }
        let before = aliases.len();
        let aliases: Vec<BaselineAlias> = aliases.into_iter().filter(|a| ids.contains(&a.from)).collect();
        let pruned = before - aliases.len();
        promotions.push(BaselinePromotion {
            run_id: input.run_id.clone(),
            at: input.now.clone(),
            summary: input.summary.clone(),
        });
        if promotions.len() > MAX_PROMOTIONS {
            promotions.drain(..promotions.len() - MAX_PROMOTIONS);
        }
        let file = BaselineFile {
            format_version: FORMAT_VERSION,
            key: name.to_string(),
            url: input.url.clone(),
            fingerprint_version: input.fingerprint_version,
            promoted_at: input.now.clone(),
            promoted_from: BaselineSource {
                run_id: input.run_id.clone(),
                engine_version: input.engine_version.clone(),
            },
            targets,
            aliases,
            promotions,
        };
        write_baseline_json(stage, &file)?;
        Ok((file, pruned))
    }
}

fn check_component_id(id: &str) -> Result<(), BaselineError> {
    let ok = id.len() == 14 && id.starts_with("c_") && id[2..].chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'));
    if ok {
        Ok(())
    } else {
        Err(BaselineError::Invalid(format!(
            "{id:?} is not a component id (c_ and 12 hex characters)"
        )))
    }
}

/// Writes `baseline.json` in `dir` through a temporary file and a rename.
fn write_baseline_json(dir: &Path, file: &BaselineFile) -> Result<(), BaselineError> {
    let mut text = serde_json::to_string_pretty(file).map_err(|e| BaselineError::Io(e.to_string()))?;
    text.push('\n');
    let temporary = dir.join(".baseline.json.tmp");
    let path = dir.join("baseline.json");
    fs::write(&temporary, text)
        .and_then(|()| fs::rename(&temporary, &path))
        .map_err(|e| {
            let _ = fs::remove_file(&temporary);
            io_error("could not write", &path, &e)
        })
}

fn copy_file(from: &Path, to: &Path) -> Result<(), BaselineError> {
    // Reading through read_json's checks first would parse twice; the copy only needs the same
    // symlink and size rules.
    check_plain_file(from)?;
    fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| io_error("could not copy", from, &e))
}

fn check_plain_file(path: &Path) -> Result<u64, BaselineError> {
    let meta = fs::symlink_metadata(path).map_err(|e| io_error("could not read", path, &e))?;
    if meta.file_type().is_symlink() {
        return Err(BaselineError::Invalid(format!(
            "{} is a symbolic link; refusing to follow it",
            path.display()
        )));
    }
    if !meta.is_file() {
        return Err(BaselineError::Invalid(format!("{} is not a file", path.display())));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(BaselineError::Invalid(format!(
            "{} is too large ({} bytes; the limit is {} MiB)",
            path.display(),
            meta.len(),
            MAX_FILE_BYTES / (1024 * 1024)
        )));
    }
    Ok(meta.len())
}

/// Copies `*.png` files (plain files only) of a crops folder. Returns whether any were copied.
fn copy_crops(from: &Path, to: &Path) -> Result<bool, BaselineError> {
    let Ok(meta) = fs::symlink_metadata(from) else {
        return Ok(false);
    };
    if !meta.is_dir() {
        return Ok(false);
    }
    let mut copied = 0usize;
    for entry in fs::read_dir(from)
        .map_err(|e| io_error("could not read", from, &e))?
        .flatten()
    {
        let path = entry.path();
        let is_png = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("png"));
        let plain = entry.file_type().is_ok_and(|t| t.is_file());
        if !is_png || !plain {
            continue;
        }
        if copied == 0 {
            fs::create_dir(to).map_err(|e| io_error("could not create", to, &e))?;
        }
        copied += 1;
        if copied > MAX_CROPS {
            return Err(BaselineError::Invalid(format!(
                "{} holds more than {MAX_CROPS} crops",
                from.display()
            )));
        }
        copy_file(&path, &to.join(entry.file_name()))?;
    }
    Ok(copied > 0)
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, BaselineError> {
    check_plain_file(path)?;
    let mut text = String::new();
    fs::File::open(path)
        .and_then(|f| f.take(MAX_FILE_BYTES + 1).read_to_string(&mut text))
        .map_err(|e| io_error("could not read", path, &e))?;
    serde_json::from_str(&text).map_err(|e| BaselineError::Invalid(format!("{} is not valid: {e}", path.display())))
}

/// Turns removed and added entries that an alias joins into one renamed pair: both get
/// `pairedWith` with reason `aliased`, which the gate and the annotations treat as accepted. An
/// alias wins over a heuristic pair, whose stale links are dropped. Returns how many pairs were
/// made. Fixes `summary.paired` to count every pair.
pub fn apply_aliases(diff: &mut DiffFile, aliases: &[BaselineAlias]) -> usize {
    use traceqa_common::types::{Confidence, DiffEntryKind};
    let mut made = 0;
    for alias in aliases {
        let from = diff
            .entries
            .iter()
            .position(|e| e.kind == DiffEntryKind::Removed && e.id == alias.from);
        let to = diff
            .entries
            .iter()
            .position(|e| e.kind == DiffEntryKind::Added && e.id == alias.to);
        let (Some(from), Some(to)) = (from, to) else { continue };
        for entry in &mut diff.entries {
            if entry
                .paired_with
                .as_ref()
                .is_some_and(|p| p.id == alias.from || p.id == alias.to)
            {
                entry.paired_with = None;
            }
        }
        let link = |id: &str| PairLink {
            id: id.to_string(),
            reason: ALIASED.to_string(),
            confidence: Confidence::High,
        };
        diff.entries[from].paired_with = Some(link(&alias.to));
        diff.entries[to].paired_with = Some(link(&alias.from));
        made += 1;
    }
    diff.summary.paired = diff
        .entries
        .iter()
        .filter(|e| e.kind == DiffEntryKind::Removed && e.paired_with.is_some())
        .count() as u64;
    made
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::{compare, Options};

    const COMPONENTS: &str = include_str!("../../../spec/examples/ComponentsFile.shop.json");

    struct Temp(PathBuf);

    impl Temp {
        fn new() -> Self {
            static N: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "traceqa-baseline-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn input(temp: &Temp, run_id: &str, components: &str, crops: bool) -> SaveInput {
        let target = temp.0.join("runs").join(run_id).join("chromium-1280x800");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("components.json"), components).unwrap();
        if crops {
            fs::create_dir_all(target.join("crops")).unwrap();
            fs::write(target.join("crops").join("c_9f2a41c07b3e.png"), b"png").unwrap();
            fs::write(target.join("crops").join("notes.txt"), b"x").unwrap();
        }
        let file: ComponentsFile = serde_json::from_str(components).unwrap();
        SaveInput {
            run_id: run_id.into(),
            engine_version: "0.1.0".into(),
            url: file.url.clone(),
            fingerprint_version: file.fingerprint_version,
            now: "2026-03-02T09:30:00Z".into(),
            summary: "first save".into(),
            targets: vec![TargetSource {
                dir: "chromium-1280x800".into(),
                path: target,
                browser: file.browser.clone(),
                viewport: file.viewport.clone(),
            }],
        }
    }

    #[test]
    fn the_key_is_the_host_and_eight_hex_of_the_address() {
        let key = key_for("https://Shop.Example.com:8443/a?token=REDACTED");
        assert!(key.starts_with("shop.example.com-"), "{key}");
        assert_eq!(key.len(), "shop.example.com-".len() + 8);
        assert_eq!(key, key_for("https://Shop.Example.com:8443/a?token=REDACTED"));
        assert_ne!(key, key_for("https://shop.example.com/b"));
        assert!(key_for("not a url").starts_with("page-"));
        let ipv6 = key_for("http://[::1]:3000/");
        assert!(check_name(&ipv6).is_ok(), "{ipv6}");
        // The key must be usable as a folder name on Windows.
        assert!(!key_for("http://a_b$c/").contains(['_', '$', ':']));
    }

    #[test]
    fn names_that_are_not_folder_names_are_refused() {
        for bad in [
            "",
            ".hidden",
            "a/b",
            "a\\b",
            "..",
            "con",
            "NUL.txt",
            "lpt1",
            &"x".repeat(65),
            "a b",
        ] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
        for good in ["main", "shop-1.2_x", "COM10"] {
            assert!(check_name(good).is_ok(), "{good:?}");
        }
    }

    #[test]
    fn save_copies_the_run_and_refuses_to_replace() {
        let temp = Temp::new();
        let store = BaselineStore::new(temp.0.join("baselines"));
        let input = input(&temp, "r_one", COMPONENTS, true);
        let file = store.save(Some("main"), &input).unwrap();
        assert_eq!(file.key, "main");
        assert!(file.targets[0].has_crops);
        let loaded = store.load("main").unwrap();
        assert_eq!(loaded.read_components("chromium-1280x800").unwrap().components.len(), 1);
        let crops = loaded.target_dir("chromium-1280x800").join("crops");
        assert!(crops.join("c_9f2a41c07b3e.png").is_file());
        assert!(!crops.join("notes.txt").exists());
        let again = store.save(Some("main"), &input).unwrap_err();
        assert!(matches!(again, BaselineError::Exists { .. }), "{again}");
        assert!(again.to_string().contains("promote"));
        // No staging folder is left behind.
        let leftovers: Vec<_> = fs::read_dir(store.root())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn a_save_that_fails_leaves_nothing() {
        let temp = Temp::new();
        let store = BaselineStore::new(temp.0.join("baselines"));
        let mut bad = input(&temp, "r_one", COMPONENTS, false);
        bad.targets[0].path = temp.0.join("nowhere");
        assert!(store.save(Some("main"), &bad).is_err());
        assert!(store.list().is_empty());
        assert_eq!(fs::read_dir(store.root()).unwrap().count(), 0);
    }

    #[test]
    fn an_unknown_name_lists_the_known_ones() {
        let temp = Temp::new();
        let store = BaselineStore::new(temp.0.join("baselines"));
        store
            .save(Some("main"), &input(&temp, "r_one", COMPONENTS, false))
            .unwrap();
        let error = store.load("mian").unwrap_err().to_string();
        assert!(error.contains("there is: main"), "{error}");
    }

    #[test]
    fn promote_replaces_records_history_and_prunes_dead_aliases() {
        let temp = Temp::new();
        let store = BaselineStore::new(temp.0.join("baselines"));
        store
            .save(Some("main"), &input(&temp, "r_one", COMPONENTS, true))
            .unwrap();
        store
            .add_alias(
                "main",
                &AliasInput {
                    from: "c_9f2a41c07b3e".into(),
                    to: "c_0a1b2c3d4e5f".into(),
                    reason: "rename".into(),
                    evidence: None,
                    now: "2026-03-03T00:00:00Z".into(),
                },
            )
            .unwrap();
        // The new run no longer has the aliased component.
        let changed = COMPONENTS.replace("c_9f2a41c07b3e", "c_0a1b2c3d4e5f");
        let mut second = input(&temp, "r_two", &changed, false);
        second.summary = "1 removed, 1 added, 0 changed".into();
        let promoted = store.promote("main", &second).unwrap();
        assert_eq!(promoted.pruned_aliases, 1);
        assert!(promoted.file.aliases.is_empty());
        assert_eq!(promoted.file.promotions.len(), 2);
        assert_eq!(promoted.file.promoted_from.run_id, "r_two");
        let loaded = store.load("main").unwrap();
        assert!(!loaded.file.targets[0].has_crops);
        assert!(!loaded.target_dir("chromium-1280x800").join("crops").exists());
        assert!(loaded.read_components("chromium-1280x800").unwrap().components[0].id == "c_0a1b2c3d4e5f");
    }

    #[test]
    fn promote_needs_an_existing_baseline() {
        let temp = Temp::new();
        let store = BaselineStore::new(temp.0.join("baselines"));
        let error = store
            .promote("main", &input(&temp, "r", COMPONENTS, false))
            .unwrap_err();
        assert!(matches!(error, BaselineError::Missing { .. }));
    }

    #[test]
    fn an_alias_must_start_from_a_baseline_component() {
        let temp = Temp::new();
        let store = BaselineStore::new(temp.0.join("baselines"));
        store
            .save(Some("main"), &input(&temp, "r_one", COMPONENTS, false))
            .unwrap();
        let alias = |from: &str, to: &str| AliasInput {
            from: from.into(),
            to: to.into(),
            reason: "manual".into(),
            evidence: Some("moved".into()),
            now: "2026-03-03T00:00:00Z".into(),
        };
        assert!(store
            .add_alias("main", &alias("c_ffffffffffff", "c_0a1b2c3d4e5f"))
            .is_err());
        assert!(store.add_alias("main", &alias("nonsense", "c_0a1b2c3d4e5f")).is_err());
        assert!(store
            .add_alias("main", &alias("c_9f2a41c07b3e", "c_9f2a41c07b3e"))
            .is_err());
        store
            .add_alias("main", &alias("c_9f2a41c07b3e", "c_0a1b2c3d4e5f"))
            .unwrap();
        let file = store
            .add_alias("main", &alias("c_9f2a41c07b3e", "c_1a1b2c3d4e5f"))
            .unwrap();
        assert_eq!(file.aliases.len(), 1);
        assert_eq!(file.aliases[0].to, "c_1a1b2c3d4e5f");
    }

    #[test]
    fn aliases_turn_removed_and_added_into_one_renamed_pair() {
        let a: ComponentsFile = serde_json::from_str(COMPONENTS).unwrap();
        let mut b = a.clone();
        b.components[0].id = "c_0a1b2c3d4e5f".into();
        let mut diff = compare(&a, &b, &Options::default()).unwrap();
        assert_eq!((diff.summary.removed, diff.summary.added), (1, 1));
        let alias = BaselineAlias {
            from: "c_9f2a41c07b3e".into(),
            to: "c_0a1b2c3d4e5f".into(),
            reason: "rename".into(),
            evidence: None,
            added_at: "2026-03-03T00:00:00Z".into(),
        };
        assert_eq!(apply_aliases(&mut diff, std::slice::from_ref(&alias)), 1);
        assert_eq!(diff.summary.paired, 1);
        assert!(diff.entries.iter().all(crate::gate::is_aliased));
        let gate = crate::Gate::parse("removed,added,paired").unwrap();
        assert_eq!(gate.count([&diff]), 0);
        // An alias to an id that is not added does nothing.
        let mut same = compare(&a, &a, &Options::default()).unwrap();
        assert_eq!(apply_aliases(&mut same, &[alias]), 0);
    }
}
