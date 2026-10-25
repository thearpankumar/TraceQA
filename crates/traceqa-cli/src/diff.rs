//! `traceqa diff A B` and `traceqa diff --against BASELINE RUN`: what differs between two finished
//! runs, or between a run and a saved baseline. Reads only; never starts a browser and never
//! writes a baseline (that is `traceqa baseline`).

use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::ExitCode;

use clap::{Args, ValueEnum};
use traceqa_common::types::{BaselineRef, BaselineReport, BaselineTargetReport, ComponentsFile, RunStatus};
use traceqa_diff::baseline::{apply_aliases, Loaded};
use traceqa_diff::{compare, github, render_markdown, sarif, BaselineStore, DiffError, DiffFile, Gate, Options};
use traceqa_recorder::{open_run, read_components, RunFolder};

/// Where `--against NAME` looks for the baseline.
pub const DEFAULT_BASELINE_DIR: &str = ".traceqa/baselines";

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Markdown,
    Json,
    Github,
    Sarif,
}

#[derive(Args)]
pub struct DiffArgs {
    /// Side A: a run folder, a target folder, a components.json, or the id of a run in --runs-dir.
    /// With --against it is the run to check, and the baseline is side A.
    a: String,
    /// Side B, in the same forms. In the report, "added" means in B only.
    #[arg(required_unless_present = "against", conflicts_with = "against")]
    b: Option<String>,
    /// Compare the run with a saved baseline: its name in --baseline-dir, or a baseline folder.
    /// Every target of the baseline is compared with the run's target of the same folder name.
    #[arg(long, value_name = "NAME|DIR")]
    against: Option<String>,
    /// The folder of baselines, for --against NAME.
    #[arg(long, value_name = "DIR", default_value = DEFAULT_BASELINE_DIR, requires = "against")]
    baseline_dir: String,
    /// Exit 1 when any entry matches one of these, comma-separated: removed, added, changed,
    /// disabled, hidden, covered, paired. A rename that a baseline alias covers matches nothing.
    #[arg(long, value_name = "TOKENS")]
    fail_on: Option<String>,
    /// The target folder to use on both sides when a run holds several (such as chromium-1280x800).
    #[arg(long, value_name = "NAME", conflicts_with_all = ["target_a", "target_b", "against"])]
    target: Option<String>,
    /// The target folder to use on side A.
    #[arg(long, value_name = "NAME", conflicts_with = "against")]
    target_a: Option<String>,
    /// The target folder to use on side B.
    #[arg(long, value_name = "NAME", conflicts_with = "against")]
    target_b: Option<String>,
    /// Report format. Markdown is for people and pull requests; json is the DiffFile contract
    /// (BaselineReport with --against); github prints workflow commands and fills
    /// $GITHUB_STEP_SUMMARY; sarif is SARIF 2.1.0 for viewers and other tools.
    #[arg(long, value_enum, default_value = "markdown")]
    format: Format,
    /// Write the report to this file instead of standard output.
    #[arg(long, value_name = "FILE")]
    out: Option<String>,
    /// Where bare run ids are looked up.
    #[arg(long, value_name = "DIR", default_value = "runs")]
    runs_dir: String,
}

/// 0 when the diff ran and no gate tripped; 1 when `--fail-on` matched an entry; 2 for input that
/// cannot be compared (also when a baseline target is missing from the run).
pub fn run(args: &DiffArgs) -> ExitCode {
    match diff(args) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("traceqa: diff: {message}");
            ExitCode::from(2)
        }
    }
}

/// One baseline target, or the single pair of an A/B diff, compared.
pub struct Outcome {
    pub dir: String,
    pub result: Result<DiffFile, String>,
}

/// Why a run could not be compared with a baseline.
pub enum CompareError {
    /// The baseline and the run use different fingerprint recipes.
    Fingerprint {
        baseline: u64,
        run: u64,
    },
    Other(String),
}

impl CompareError {
    pub fn message(&self, baseline_name: &str) -> String {
        match self {
            Self::Fingerprint { baseline, run } => format!(
                "the baseline uses fingerprint version {baseline} and the run {run}, so their component ids cannot \
                 be compared; re-save the baseline from a current run: traceqa baseline promote RUN --name \
                 {baseline_name} --yes"
            ),
            Self::Other(message) => message.clone(),
        }
    }
}

/// Compares a run with a baseline, target by target. A baseline target that the run lacks, or that
/// did not finish, is an error outcome; a run target that the baseline lacks is ignored with a note.
pub fn against(loaded: &Loaded, run: &RunFolder, run_label: &str) -> Result<Vec<Outcome>, CompareError> {
    let base_label = format!("baseline {}", loaded.file.key);
    let mut outcomes = Vec::new();
    let mut files = Vec::new();
    for saved in &loaded.file.targets {
        let Some(target) = run.targets.iter().find(|t| t.dir == saved.dir) else {
            let have: Vec<&str> = run.targets.iter().map(|t| t.dir.as_str()).collect();
            outcomes.push(Outcome {
                dir: saved.dir.clone(),
                result: Err(format!(
                    "the baseline has target {} but the run does not; the run has: {}",
                    saved.dir,
                    have.join(", ")
                )),
            });
            continue;
        };
        if let Some(status) = target
            .summary
            .as_ref()
            .map(|s| s.status)
            .filter(|s| *s != RunStatus::Completed)
        {
            outcomes.push(Outcome {
                dir: saved.dir.clone(),
                result: Err(format!("the run's target {} did not finish ({status})", saved.dir)),
            });
            continue;
        }
        let new = read_components(target).map_err(|e| CompareError::Other(e.to_string()))?;
        if new.fingerprint_version != loaded.file.fingerprint_version {
            return Err(CompareError::Fingerprint {
                baseline: loaded.file.fingerprint_version,
                run: new.fingerprint_version,
            });
        }
        files.push((saved.dir.clone(), new));
    }
    for target in &run.targets {
        if !loaded.file.targets.iter().any(|t| t.dir == target.dir) {
            eprintln!(
                "traceqa: note: the baseline has no target {}, so it is not checked",
                target.dir
            );
        }
    }
    for (dir, new) in files {
        let old = loaded
            .read_components(&dir)
            .map_err(|e| CompareError::Other(e.to_string()))?;
        let options = Options {
            label_a: base_label.clone(),
            label_b: run_label.to_string(),
            target_a: Some(dir.clone()),
            target_b: Some(dir.clone()),
        };
        let result = match compare(&old, &new, &options) {
            Ok(mut diff) => {
                apply_aliases(&mut diff, &loaded.file.aliases);
                Ok(diff)
            }
            Err(DiffError::FingerprintVersion { a, b }) => {
                return Err(CompareError::Fingerprint { baseline: a, run: b });
            }
        };
        outcomes.push(Outcome { dir, result });
    }
    outcomes.sort_by(|x, y| x.dir.cmp(&y.dir));
    Ok(outcomes)
}

/// Opens the baseline named by `--against`: a folder that holds `baseline.json` wins over a name.
pub fn open_baseline(against: &str, dir: &str) -> Result<Loaded, String> {
    let path = Path::new(against);
    if path.join("baseline.json").is_file() {
        return BaselineStore::load_dir(path).map_err(|e| e.to_string());
    }
    BaselineStore::new(dir).load(against).map_err(|e| e.to_string())
}

fn diff(args: &DiffArgs) -> Result<ExitCode, String> {
    let gate = match &args.fail_on {
        Some(text) => Gate::parse(text).map_err(|e| e.to_string())?,
        None => Gate::default(),
    };
    let runs_dir = Path::new(&args.runs_dir);
    let open = |arg: &str| open_run(arg, runs_dir).map_err(|e| e.to_string());

    let (outcomes, baseline) = match (&args.against, &args.b) {
        (Some(against_arg), _) => {
            let loaded = open_baseline(against_arg, &args.baseline_dir)?;
            let run = open(&args.a)?;
            match against(&loaded, &run, &args.a) {
                Ok(outcomes) => (outcomes, Some(loaded)),
                Err(e) => return Err(e.message(&loaded.file.key)),
            }
        }
        (None, Some(b)) => {
            let (run_a, run_b) = (open(&args.a)?, open(b)?);
            let name_a = args.target_a.as_ref().or(args.target.as_ref());
            let name_b = args.target_b.as_ref().or(args.target.as_ref());
            let (dir_a, file_a) = pick(&run_a, name_a, "A")?;
            let (dir_b, file_b) = pick(&run_b, name_b, "B")?;
            let options = Options {
                label_a: args.a.clone(),
                label_b: b.clone(),
                target_a: Some(dir_a),
                target_b: Some(dir_b.clone()),
            };
            let result = compare(&file_a, &file_b, &options).map_err(|e| e.to_string())?;
            (
                vec![Outcome {
                    dir: dir_b,
                    result: Ok(result),
                }],
                None,
            )
        }
        (None, None) => return Err("give two runs to compare, or one run with --against".into()),
    };

    let diffs: Vec<&DiffFile> = outcomes.iter().filter_map(|o| o.result.as_ref().ok()).collect();
    let matched = gate.count(diffs.iter().copied());
    let broken = outcomes.iter().any(|o| o.result.is_err());

    let markdown = markdown_report(&outcomes, &gate, matched);
    let view: Vec<(&str, Result<&DiffFile, &str>)> = outcomes
        .iter()
        .map(|o| (o.dir.as_str(), o.result.as_ref().map_err(String::as_str)))
        .collect();
    let url = baseline
        .as_ref()
        .map(|b| b.file.url.clone())
        .or_else(|| diffs.first().map(|d| d.b.url.clone()))
        .unwrap_or_default();
    let mut text = match args.format {
        Format::Markdown => markdown.clone(),
        Format::Github => github::render(&github::diff_annotations(&view)),
        Format::Sarif => serde_json::to_string_pretty(&sarif::diff_log(&url, &view)).map_err(|e| e.to_string())?,
        Format::Json => match &baseline {
            Some(loaded) => {
                let report = BaselineReport {
                    format_version: 1,
                    baseline: BaselineRef {
                        key: loaded.file.key.clone(),
                        url: loaded.file.url.clone(),
                        promoted_at: loaded.file.promoted_at.clone(),
                    },
                    fail_on: gate.names(),
                    failed: matched > 0,
                    matched: matched as u64,
                    targets: outcomes
                        .iter()
                        .map(|o| BaselineTargetReport {
                            dir: o.dir.clone(),
                            diff: o.result.as_ref().ok().cloned(),
                            error: o.result.as_ref().err().cloned(),
                        })
                        .collect(),
                };
                serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
            }
            None => match &outcomes[0].result {
                Ok(only) => serde_json::to_string_pretty(only).map_err(|e| e.to_string())?,
                Err(why) => return Err(why.clone()),
            },
        },
    };
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    match &args.out {
        Some(path) => fs::write(path, text).map_err(|e| format!("--out {path}: {e}"))?,
        None => print!("{text}"),
    }
    if matches!(args.format, Format::Github) {
        append_step_summary(&markdown);
    }

    Ok(if broken {
        ExitCode::from(2)
    } else if matched > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// Markdown for every target, then the errors and the result of the gate.
fn markdown_report(outcomes: &[Outcome], gate: &Gate, matched: usize) -> String {
    let mut out = String::new();
    for outcome in outcomes {
        match &outcome.result {
            Ok(diff) => {
                out.push_str(&render_markdown(diff));
                out.push('\n');
            }
            Err(why) => {
                let _ = writeln!(
                    out,
                    "## TraceQA diff\n\n> **Error** target `{}`: {}\n",
                    traceqa_diff::text::plain(&outcome.dir).replace('`', ""),
                    traceqa_diff::text::plain(why)
                );
            }
        }
    }
    if !gate.is_empty() {
        let _ = writeln!(
            out,
            "**Gate** (`--fail-on {}`): {}\n",
            gate.names().join(","),
            if matched == 0 {
                "passed".to_string()
            } else {
                format!("failed, {matched} matching entries")
            }
        );
    }
    out
}

/// GitHub Actions gives the path of a file whose Markdown shows on the run's summary page.
fn append_step_summary(markdown: &str) {
    let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY").filter(|p| !p.is_empty()) else {
        return;
    };
    let written = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(markdown.as_bytes()));
    if let Err(e) = written {
        eprintln!("traceqa: warning: could not write $GITHUB_STEP_SUMMARY: {e}");
    }
}

/// The one target of a side, read. A run with several targets needs one to be named.
fn pick(run: &RunFolder, name: Option<&String>, side: &str) -> Result<(String, ComponentsFile), String> {
    let names = || {
        run.targets
            .iter()
            .map(|t| t.dir.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let target = match name {
        Some(name) => run
            .targets
            .iter()
            .find(|t| &t.dir == name)
            .ok_or_else(|| format!("side {side} has no target {name}; it has: {}", names()))?,
        None => match run.targets.as_slice() {
            [only] => only,
            [] => return Err(format!("side {side} has no targets")),
            _ => {
                return Err(format!(
                    "side {side} has several targets ({}); name one with --target (the same name on both sides), \
                     --target-a or --target-b",
                    names()
                ))
            }
        },
    };
    let file = read_components(target).map_err(|e| e.to_string())?;
    Ok((target.dir.clone(), file))
}
