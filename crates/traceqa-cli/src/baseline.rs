//! `traceqa baseline`: keep a reference run to compare later runs with.
//!
//! - `save` makes a baseline from a run and refuses to replace one.
//! - `promote` replaces a baseline with a run, only with `--yes`. Without it the command shows what
//!   would change and exits 2, so a CI job can never move the reference by accident.
//! - `alias` records that a component was renamed, so a diff reports the pair as renamed.
//! - `init` writes a manifest (the format of `testbed/sites/buggy-shop/manifest.yaml`) from a run.
//!
//! `extract` and `diff` never write a baseline.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use clap::{Args, Subcommand};
use traceqa_common::types::{ComponentsFile, RunStatus};
use traceqa_diff::baseline::{key_for, AliasInput, Loaded, SaveInput, TargetSource};
use traceqa_diff::{render_markdown, BaselineStore};
use traceqa_recorder::{now_rfc3339, open_run, read_components, RunFolder};

use crate::diff::{against, CompareError, DEFAULT_BASELINE_DIR};
use crate::manifest;

#[derive(Args)]
pub struct BaselineArgs {
    #[command(subcommand)]
    command: Sub,
}

#[derive(Args)]
struct Where {
    /// A run folder, a target folder, a components.json, or the id of a run in --runs-dir.
    run: String,
    /// The baseline's name. Default: the page's host and 8 hex characters of its address.
    #[arg(long, value_name = "NAME")]
    name: Option<String>,
    /// The folder of baselines. It may be committed.
    #[arg(long, value_name = "DIR", default_value = DEFAULT_BASELINE_DIR)]
    dir: String,
    /// Where bare run ids are looked up.
    #[arg(long, value_name = "DIR", default_value = "runs")]
    runs_dir: String,
}

#[derive(Subcommand)]
enum Sub {
    /// Make a baseline from a run. Fails when the baseline exists; use `promote` to replace it.
    Save(Where),
    /// Replace a baseline with a run. Without --yes, show the difference and exit 2.
    Promote {
        #[command(flatten)]
        at: Where,
        /// Replace the baseline. There is no prompt: this is the explicit step.
        #[arg(long)]
        yes: bool,
    },
    /// Record that a component of the baseline was renamed to a component of the next run.
    Alias {
        /// The component id in the baseline.
        from: String,
        /// The component id in the run.
        to: String,
        /// The baseline's name.
        #[arg(long, value_name = "NAME")]
        name: String,
        /// The folder of baselines.
        #[arg(long, value_name = "DIR", default_value = DEFAULT_BASELINE_DIR)]
        dir: String,
        /// Why they are the same component.
        #[arg(long, value_name = "TEXT")]
        evidence: Option<String>,
        /// Known values: rename, heal, manual.
        #[arg(long, value_name = "REASON", default_value = "rename")]
        reason: String,
    },
    /// Write a manifest from a run, to edit and keep. Lists what was found, per viewport.
    Init {
        /// The run, in the forms `save` takes.
        run: String,
        /// The file to write.
        #[arg(long, value_name = "FILE")]
        out: String,
        /// Name a viewport, as 1280x800=desktop. Repeat. Unnamed viewports are called WIDTHxHEIGHT.
        #[arg(long = "viewport-name", value_name = "WxH=NAME")]
        viewport_names: Vec<String>,
        /// Replace the file when it exists.
        #[arg(long)]
        force: bool,
        /// Where bare run ids are looked up.
        #[arg(long, value_name = "DIR", default_value = "runs")]
        runs_dir: String,
    },
}

pub fn run(args: &BaselineArgs) -> ExitCode {
    let result = match &args.command {
        Sub::Save(at) => save(at),
        Sub::Promote { at, yes } => promote(at, *yes),
        Sub::Alias {
            from,
            to,
            name,
            dir,
            evidence,
            reason,
        } => alias(from, to, name, dir, evidence.as_deref(), reason),
        Sub::Init {
            run,
            out,
            viewport_names,
            force,
            runs_dir,
        } => init(run, out, viewport_names, *force, runs_dir),
    };
    match result {
        Ok(code) => code,
        Err(message) => {
            eprintln!("traceqa: baseline: {message}");
            ExitCode::from(2)
        }
    }
}

/// The finished targets of a run, read.
fn finished(run: &RunFolder) -> Result<Vec<(String, std::path::PathBuf, ComponentsFile)>, String> {
    let mut found = Vec::new();
    for target in &run.targets {
        if target
            .summary
            .as_ref()
            .is_some_and(|s| s.status != RunStatus::Completed)
        {
            eprintln!("traceqa: note: skipping {}: it did not finish", target.dir);
            continue;
        }
        let file = read_components(target).map_err(|e| e.to_string())?;
        found.push((target.dir.clone(), target.path.clone(), file));
    }
    if found.is_empty() {
        return Err("the run has no finished target".into());
    }
    Ok(found)
}

fn save_input(run: &RunFolder, summary: &str) -> Result<SaveInput, String> {
    let targets = finished(run)?;
    let first = &targets[0].2;
    for (dir, _, file) in &targets {
        if file.url != first.url || file.fingerprint_version != first.fingerprint_version {
            return Err(format!(
                "target {dir} was read from another page or with another fingerprint version than {}; \
                 a baseline is made from one run of one page",
                targets[0].0
            ));
        }
    }
    Ok(SaveInput {
        run_id: run
            .manifest
            .as_ref()
            .map_or_else(|| first.run_id.clone(), |m| m.run_id.clone()),
        engine_version: run
            .manifest
            .as_ref()
            .map_or_else(|| "unknown".to_string(), |m| m.engine_version.clone()),
        url: first.url.clone(),
        fingerprint_version: first.fingerprint_version,
        now: now_rfc3339(),
        summary: summary.to_string(),
        targets: targets
            .iter()
            .map(|(dir, path, file)| TargetSource {
                dir: dir.clone(),
                path: path.clone(),
                browser: file.browser.clone(),
                viewport: file.viewport.clone(),
            })
            .collect(),
    })
}

fn save(at: &Where) -> Result<ExitCode, String> {
    let run = open_run(&at.run, Path::new(&at.runs_dir)).map_err(|e| e.to_string())?;
    let input = save_input(&run, "first save")?;
    let file = BaselineStore::new(&at.dir)
        .save(at.name.as_deref(), &input)
        .map_err(|e| e.to_string())?;
    println!(
        "Saved baseline {} ({} targets) in {}",
        file.key,
        file.targets.len(),
        at.dir
    );
    println!(
        "Check later runs with: traceqa diff --against {} RUN --fail-on removed,disabled",
        file.key
    );
    Ok(ExitCode::SUCCESS)
}

/// A one-line summary of a baseline-against-run comparison.
fn summarise(loaded: &Loaded, run: &RunFolder, label: &str) -> (String, String) {
    match against(loaded, run, label) {
        Ok(outcomes) => {
            let (mut removed, mut added, mut changed) = (0, 0, 0);
            let mut markdown = String::new();
            for o in &outcomes {
                match &o.result {
                    Ok(d) => {
                        removed += d.summary.removed;
                        added += d.summary.added;
                        changed += d.summary.changed;
                        markdown.push_str(&render_markdown(d));
                    }
                    Err(why) => markdown.push_str(&format!("> Target {}: {why}\n\n", o.dir.replace('`', ""))),
                }
            }
            (format!("{removed} removed, {added} added, {changed} changed"), markdown)
        }
        Err(e @ CompareError::Fingerprint { .. }) => (
            format!(
                "fingerprint version changed; ids cannot be compared ({})",
                e.message("NAME")
            ),
            String::new(),
        ),
        Err(CompareError::Other(why)) => (format!("not compared: {why}"), String::new()),
    }
}

fn promote(at: &Where, yes: bool) -> Result<ExitCode, String> {
    let run = open_run(&at.run, Path::new(&at.runs_dir)).map_err(|e| e.to_string())?;
    let mut input = save_input(&run, "")?;
    let name = at.name.clone().unwrap_or_else(|| key_for(&input.url));
    let store = BaselineStore::new(&at.dir);
    let loaded = store.load(&name).map_err(|e| e.to_string())?;
    if loaded.file.url != input.url {
        eprintln!(
            "traceqa: note: baseline {name} was made from {}, this run is of {}",
            loaded.file.url, input.url
        );
    }
    let (summary, markdown) = summarise(&loaded, &run, &at.run);
    if !yes {
        print!("{markdown}");
        println!("Baseline {name} would change: {summary}.");
        eprintln!(
            "traceqa: baseline: not promoted. Review the difference above, then run the same command with --yes. \
             A baseline is never replaced without it."
        );
        return Ok(ExitCode::from(2));
    }
    input.summary = summary.clone();
    let promoted = store.promote(&name, &input).map_err(|e| e.to_string())?;
    println!(
        "Promoted baseline {name} to run {}: {summary}.",
        promoted.file.promoted_from.run_id
    );
    if promoted.pruned_aliases > 0 {
        println!(
            "Dropped {} alias(es) whose old component is gone.",
            promoted.pruned_aliases
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn alias(
    from: &str,
    to: &str,
    name: &str,
    dir: &str,
    evidence: Option<&str>,
    reason: &str,
) -> Result<ExitCode, String> {
    let input = AliasInput {
        from: from.to_string(),
        to: to.to_string(),
        reason: reason.to_string(),
        evidence: evidence.map(str::to_string),
        now: now_rfc3339(),
    };
    BaselineStore::new(dir)
        .add_alias(name, &input)
        .map_err(|e| e.to_string())?;
    println!("Baseline {name}: {from} is now treated as {to} when a run has {to}.");
    Ok(ExitCode::SUCCESS)
}

fn init(run: &str, out: &str, names: &[String], force: bool, runs_dir: &str) -> Result<ExitCode, String> {
    let mut viewport_names = BTreeMap::new();
    for text in names {
        let (size, name) = text
            .split_once('=')
            .ok_or_else(|| format!("--viewport-name {text}: expected WxH=NAME, such as 1280x800=desktop"))?;
        let viewport = crate::args::parse_viewport(size).map_err(|e| format!("--viewport-name {text}: {e}"))?;
        if name.is_empty() || name.contains(['\n', '\r']) {
            return Err(format!("--viewport-name {text}: the name is empty"));
        }
        viewport_names.insert((viewport.width, viewport.height), name.to_string());
    }
    if !force && Path::new(out).exists() {
        return Err(format!("{out} exists; pass --force to replace it"));
    }
    let run = open_run(run, Path::new(runs_dir)).map_err(|e| e.to_string())?;
    let files: Vec<ComponentsFile> = finished(&run)?.into_iter().map(|(_, _, f)| f).collect();
    let built = manifest::build(&files, &viewport_names);
    let source = format!("run {}", files[0].run_id);
    let yaml = manifest::to_yaml(&built, &source);
    std::fs::write(out, yaml).map_err(|e| format!("could not write {out}: {e}"))?;
    println!(
        "Wrote {out}: {} components, {} viewports. Review it before relying on it.",
        built.components.len(),
        built.viewports.len()
    );
    Ok(ExitCode::SUCCESS)
}
