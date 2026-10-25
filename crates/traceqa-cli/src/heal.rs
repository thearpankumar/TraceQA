//! `traceqa heal RUN --against BASELINE`: what a run lost against a baseline, sorted into what can
//! be fixed mechanically, what needs a person, and what is a failure. Reads the run and the
//! baseline, never starts a browser, and changes the baseline only for `--accept`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::ExitCode;

use clap::{Args, ValueEnum};
use traceqa_common::types::{BaselineRef, Component, ComponentsFile, RunStatus};
use traceqa_diff::baseline::AliasInput;
use traceqa_diff::BaselineStore;
use traceqa_heal::patch::{self, Edit, Lang};
use traceqa_heal::{build_report, classify, render_markdown, HealItem, HealKind, HealReport};
use traceqa_recorder::{now_rfc3339, open_run, read_components};

use crate::diff::{open_baseline, DEFAULT_BASELINE_DIR};

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Markdown,
    Json,
}

#[derive(Args)]
pub struct HealArgs {
    /// The run to check: a run folder, a target folder, or the id of a run in --runs-dir.
    run: String,
    /// The baseline: its name in --baseline-dir, or a baseline folder.
    #[arg(long, value_name = "NAME|DIR")]
    against: String,
    /// The folder of baselines, for --against NAME.
    #[arg(long, value_name = "DIR", default_value = DEFAULT_BASELINE_DIR)]
    baseline_dir: String,
    /// Look at this target folder of the baseline only (such as chromium-1280x800).
    #[arg(long, value_name = "NAME")]
    target: Option<String>,
    /// A page object made by `traceqa codegen` (.ts or .py) to patch. Repeat for several. Only the
    /// `return` line of a property marked `traceqa:id` is changed, and only when it still holds what
    /// codegen wrote; the diff is printed first. Mechanical locator fixes are written; a rename is
    /// written only with --accept.
    #[arg(long, value_name = "PATH")]
    page_object: Vec<String>,
    /// Accept the rename proposal for this baseline component id: record it as an alias in the
    /// baseline and patch the page objects. A person's decision; repeat for several.
    #[arg(long, value_name = "ID")]
    accept: Vec<String>,
    /// Allow --accept and writing page objects when CI=true or GITHUB_ACTIONS=true. Without it CI can only report.
    #[arg(long)]
    allow_in_ci: bool,
    /// Report format: markdown for people, json for the HealReport contract (spec/heal.schema.json).
    #[arg(long, value_enum, default_value = "markdown")]
    format: Format,
    /// Where bare run ids are looked up.
    #[arg(long, value_name = "DIR", default_value = "runs")]
    runs_dir: String,
}

/// 0 when nothing needs a decision (every item is `none` or `fix`, or a proposal was accepted); 1
/// when something is a failure, a proposal or refused; 2 for input that cannot be used.
pub fn run(args: &HealArgs) -> ExitCode {
    match heal(args) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("traceqa: heal: {message}");
            ExitCode::from(2)
        }
    }
}

fn env_true(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v.eq_ignore_ascii_case("true") || v == "1")
}

/// One baseline target read together with the run's target of the same name.
struct Pair {
    dir: String,
    old: ComponentsFile,
    new: ComponentsFile,
}

fn heal(args: &HealArgs) -> Result<ExitCode, String> {
    let writes = !args.accept.is_empty() || !args.page_object.is_empty();
    if writes && !args.allow_in_ci && (env_true("CI") || env_true("GITHUB_ACTIONS")) {
        return Err(
            "--accept and --page-object write files, and this is CI (CI or GITHUB_ACTIONS is true), where heal only \
             reports; run it on your machine, or add --allow-in-ci"
                .into(),
        );
    }
    let loaded = open_baseline(&args.against, &args.baseline_dir)?;
    let run = open_run(&args.run, Path::new(&args.runs_dir)).map_err(|e| e.to_string())?;

    let mut pairs = Vec::new();
    for saved in &loaded.file.targets {
        if args.target.as_ref().is_some_and(|t| *t != saved.dir) {
            continue;
        }
        let Some(target) = run.targets.iter().find(|t| t.dir == saved.dir) else {
            let have: Vec<&str> = run.targets.iter().map(|t| t.dir.as_str()).collect();
            return Err(format!(
                "the baseline has target {} but the run does not; the run has: {}",
                saved.dir,
                have.join(", ")
            ));
        };
        if let Some(status) = target
            .summary
            .as_ref()
            .map(|s| s.status)
            .filter(|s| *s != RunStatus::Completed)
        {
            return Err(format!("the run's target {} did not finish ({status})", saved.dir));
        }
        pairs.push(Pair {
            dir: saved.dir.clone(),
            old: loaded.read_components(&saved.dir).map_err(|e| e.to_string())?,
            new: read_components(target).map_err(|e| e.to_string())?,
        });
    }
    if pairs.is_empty() {
        return Err(match &args.target {
            Some(t) => format!("the baseline has no target {t}"),
            None => "the baseline has no targets".into(),
        });
    }

    let mut items: Vec<HealItem> = Vec::new();
    for pair in &pairs {
        let found = classify(&pair.old, &pair.new, &loaded.file.aliases, &pair.dir).map_err(|e| e.to_string())?;
        items.extend(found);
    }

    // What a person accepted: validated before anything is written.
    let accepts = chosen(args, &items)?;

    let mut notes: Vec<String> = Vec::new();
    for (id, new_id, evidence) in &accepts {
        let parent = loaded
            .dir
            .parent()
            .ok_or_else(|| "the baseline folder has no parent folder".to_string())?;
        let name = loaded
            .dir
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| "the baseline folder has no usable name".to_string())?;
        BaselineStore::new(parent)
            .add_alias(
                name,
                &AliasInput {
                    from: id.clone(),
                    to: new_id.clone(),
                    reason: "heal".to_string(),
                    evidence: Some(evidence.clone()),
                    now: now_rfc3339(),
                },
            )
            .map_err(|e| e.to_string())?;
        notes.push(format!(
            "recorded alias {id} -> {new_id} in baseline {}",
            loaded.file.key
        ));
        for item in items
            .iter_mut()
            .filter(|i| i.id == *id && i.kind == HealKind::RenameProposal)
        {
            item.accepted = Some(true);
        }
    }

    for path in &args.page_object {
        let more = patch_page_object(path, &pairs, &items, &accepts)?;
        notes.extend(more);
    }

    let report = build_report(
        &run_id(&run, &pairs),
        BaselineRef {
            key: loaded.file.key.clone(),
            url: loaded.file.url.clone(),
            promoted_at: loaded.file.promoted_at.clone(),
        },
        items,
    );
    write_report(&run.path, &report)?;
    let mut text = match args.format {
        Format::Markdown => render_markdown(&report),
        Format::Json => serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?,
    };
    if !text.ends_with('\n') {
        text.push('\n');
    }
    print!("{text}");
    for note in &notes {
        eprintln!("traceqa: {note}");
    }
    if report.summary.failed
        && report
            .items
            .iter()
            .any(|i| i.kind == HealKind::RenameProposal && i.accepted != Some(true))
    {
        eprintln!(
            "traceqa: a rename is only applied when you accept it: traceqa heal {} --against {} --accept ID",
            args.run, args.against
        );
    }
    Ok(if report.summary.failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn run_id(run: &traceqa_recorder::RunFolder, pairs: &[Pair]) -> String {
    run.manifest
        .as_ref()
        .map_or_else(|| pairs[0].new.run_id.clone(), |m| m.run_id.clone())
}

/// `heal.json` goes next to `run.json`.
fn write_report(folder: &Path, report: &HealReport) -> Result<(), String> {
    let path = folder.join("heal.json");
    let mut text = serde_json::to_string_pretty(report).map_err(|e| e.to_string())?;
    text.push('\n');
    fs::write(&path, text).map_err(|e| format!("could not write {}: {e}", path.display()))
}

/// The `--accept` ids as (from, to, evidence), each backed by exactly one proposal that agrees across targets.
fn chosen(args: &HealArgs, items: &[HealItem]) -> Result<Vec<(String, String, String)>, String> {
    let mut out: Vec<(String, String, String)> = Vec::new();
    for id in &args.accept {
        if out.iter().any(|(from, _, _)| from == id) {
            continue;
        }
        let proposals: Vec<&HealItem> = items
            .iter()
            .filter(|i| i.id == *id && i.kind == HealKind::RenameProposal)
            .collect();
        let Some(first) = proposals.first() else {
            let kind = items
                .iter()
                .find(|i| i.id == *id)
                .map_or("nothing".to_string(), |i| i.kind.to_string());
            return Err(format!(
                "--accept {id}: there is no rename proposal for this id (it is {kind}); only a proposal can be accepted"
            ));
        };
        let targets: BTreeSet<&str> = proposals.iter().filter_map(|i| i.new_id.as_deref()).collect();
        if targets.len() != 1 {
            return Err(format!(
                "--accept {id}: the targets propose different new ids ({}); extract one browser at a time or fix it \
                 by hand with traceqa baseline alias",
                targets.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
        let new_id = first.new_id.clone().unwrap_or_default();
        let evidence = format!(
            "accepted by a person after heal proposed it: {}",
            first
                .evidence
                .iter()
                .map(|e| traceqa_diff::text::plain(e))
                .collect::<Vec<_>>()
                .join("; ")
        );
        out.push((id.clone(), new_id, evidence));
    }
    Ok(out)
}

/// Patches one page object. Prints the diff first, then writes it. Returns notes for the user.
fn patch_page_object(
    path: &str,
    pairs: &[Pair],
    items: &[HealItem],
    accepts: &[(String, String, String)],
) -> Result<Vec<String>, String> {
    let lang = Lang::of_path(path).ok_or_else(|| format!("--page-object {path}: expected a .ts or .py file"))?;
    let source = fs::read_to_string(path).map_err(|e| format!("--page-object {path}: could not read: {e}"))?;

    // For every id to rewrite: what each target had before and has now.
    let mut wanted: BTreeMap<String, Wanted> = BTreeMap::new();
    for item in items {
        let new_id = match item.kind {
            HealKind::LocatorFix => None,
            HealKind::RenameProposal if accepts.iter().any(|(from, _, _)| *from == item.id) => item.new_id.clone(),
            _ => continue,
        };
        let Some(pair) = pairs.iter().find(|p| Some(&p.dir) == item.target.as_ref()) else {
            continue;
        };
        let to = item.new_id.as_deref().unwrap_or(&item.id);
        let (Some(old), Some(new)) = (component(&pair.old, &item.id), component(&pair.new, to)) else {
            continue;
        };
        let (Some(was), Some(now)) = (&old.locator, &new.locator) else {
            continue;
        };
        let entry = wanted.entry(item.id.clone()).or_insert((new_id, Vec::new()));
        entry
            .1
            .push((patch::expression(was, lang), patch::expression(now, lang)));
    }

    let mut edits = Vec::new();
    let mut notes = Vec::new();
    for (id, (new_id, expressions)) in wanted {
        let news: BTreeSet<&String> = expressions.iter().map(|(_, new)| new).collect();
        if news.len() != 1 {
            notes.push(format!(
                "{path}: {id} is not patched: the targets need different locators, so a person has to choose"
            ));
            continue;
        }
        edits.push(Edit {
            id,
            old: expressions.iter().map(|(old, _)| old.clone()).collect(),
            new: expressions[0].1.clone(),
            new_id,
        });
    }
    let patched = patch::patch(&source, lang, &edits);
    for (id, why) in &patched.skipped {
        notes.push(format!("{path}: {id} is left as it is: {why}"));
    }
    let diff = patch::unified_diff(path, &source, &patched.text);
    if diff.is_empty() {
        notes.push(format!("{path}: nothing to patch"));
        return Ok(notes);
    }
    // The diff goes to stderr so that stdout stays the report (valid JSON with --format json).
    eprint!("{diff}");
    fs::write(path, &patched.text).map_err(|e| format!("--page-object {path}: could not write: {e}"))?;
    notes.push(format!("{path}: patched {}", patched.patched.join(", ")));
    Ok(notes)
}

/// The new marker id (for an accepted rename) and, per target, the expression before and after.
type Wanted = (Option<String>, Vec<(String, String)>);

fn component<'a>(file: &'a ComponentsFile, id: &str) -> Option<&'a Component> {
    file.components.iter().find(|c| c.id == id)
}
