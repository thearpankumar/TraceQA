//! `traceqa visual A B` and `traceqa visual --against BASELINE RUN`: the component crops of two
//! runs compared pixel by pixel, for the components a structural diff found unchanged. Reads only
//! the run folders; the diff pictures go to --out.

use std::path::Path;
use std::process::ExitCode;

use clap::{Args, ValueEnum};
use traceqa_protocol::types::ComponentsFile;
use traceqa_recorder::{open_run, read_components, RunFolder};
use traceqa_visual::{render_markdown, visual, Settings, Target, DEFAULT_MAX_DIFF_RATIO, DEFAULT_THRESHOLD};

use crate::diff::{open_baseline, DEFAULT_BASELINE_DIR};

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Markdown,
    Json,
}

#[derive(Args)]
pub struct VisualArgs {
    /// Side A: a run folder, a target folder, or the id of a run in --runs-dir. With --against it
    /// is the run to check, and the baseline is side A. Both runs need crops (extract --crops).
    a: String,
    /// Side B, in the same forms.
    #[arg(required_unless_present = "against", conflicts_with = "against")]
    b: Option<String>,
    /// Compare the run with a saved baseline (its name in --baseline-dir, or a baseline folder).
    /// The baseline must have been saved from a run with crops.
    #[arg(long, value_name = "NAME|DIR")]
    against: Option<String>,
    /// The folder of baselines, for --against NAME.
    #[arg(long, value_name = "DIR", default_value = DEFAULT_BASELINE_DIR, requires = "against")]
    baseline_dir: String,
    /// The target folder to use on both sides when a run holds several (such as chromium-1280x800).
    #[arg(long, value_name = "NAME")]
    target: Option<String>,
    /// How far two pixels' colours may differ, 0 to 1 (as in Playwright's toHaveScreenshot).
    #[arg(long, value_name = "N", default_value_t = DEFAULT_THRESHOLD)]
    threshold: f64,
    /// A component is the same when no more than this share of its pixels differ, 0 to 1.
    #[arg(long, value_name = "RATIO", default_value_t = DEFAULT_MAX_DIFF_RATIO)]
    max_diff_ratio: f64,
    /// Where the diff pictures are written: <DIR>/<target>/<id>.diff.png.
    #[arg(long, value_name = "DIR", default_value = "./visual-diff")]
    out: String,
    /// Report format on standard output. json is the VisualReport contract.
    #[arg(long, value_enum, default_value = "markdown")]
    format: Format,
    /// Exit 1 when any component is different.
    #[arg(long)]
    fail_on_different: bool,
    /// Where bare run ids are looked up.
    #[arg(long, value_name = "DIR", default_value = "runs")]
    runs_dir: String,
}

/// 0 when the comparison ran; 1 when --fail-on-different found a different component; 2 for input
/// that cannot be compared (another browser or viewport, no crops, a missing target).
pub fn run(args: &VisualArgs) -> ExitCode {
    match visual_command(args) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("traceqa: visual: {message}");
            ExitCode::from(2)
        }
    }
}

/// One side, read: where its crops are, its label and its components.
struct Side {
    label: String,
    dir: std::path::PathBuf,
    name: String,
    file: ComponentsFile,
}

impl Side {
    fn target(&self) -> Target<'_> {
        Target {
            label: &self.label,
            dir: &self.dir,
            name: Some(&self.name),
            file: &self.file,
        }
    }
}

fn visual_command(args: &VisualArgs) -> Result<ExitCode, String> {
    if !(0.0..=1.0).contains(&args.threshold) {
        return Err("--threshold must be between 0 and 1".into());
    }
    if !(0.0..=1.0).contains(&args.max_diff_ratio) {
        return Err("--max-diff-ratio must be between 0 and 1".into());
    }
    let runs_dir = Path::new(&args.runs_dir);
    let open = |arg: &str| open_run(arg, runs_dir).map_err(|e| e.to_string());
    let target = args.target.as_deref();

    let (side_a, side_b) = match (&args.against, &args.b) {
        (Some(against), _) => {
            let loaded = open_baseline(against, &args.baseline_dir)?;
            let run = open(&args.a)?;
            let saved = match target {
                Some(name) => loaded.file.targets.iter().find(|t| t.dir == name),
                None => match loaded.file.targets.as_slice() {
                    [only] => Some(only),
                    _ => None,
                },
            }
            .ok_or_else(|| {
                let have: Vec<&str> = loaded.file.targets.iter().map(|t| t.dir.as_str()).collect();
                format!(
                    "name one baseline target with --target; the baseline has: {}",
                    have.join(", ")
                )
            })?;
            if !saved.has_crops {
                return Err(format!(
                    "baseline {} was saved without crops for {}; extract with --crops, then save the baseline again",
                    loaded.file.key, saved.dir
                ));
            }
            let file_a = loaded.read_components(&saved.dir).map_err(|e| e.to_string())?;
            let (_, dir_b, file_b) = pick(&run, Some(&saved.dir), "B")?;
            (
                Side {
                    label: format!("baseline {}", loaded.file.key),
                    dir: loaded.target_dir(&saved.dir),
                    name: saved.dir.clone(),
                    file: file_a,
                },
                Side {
                    label: args.a.clone(),
                    dir: dir_b,
                    name: saved.dir.clone(),
                    file: file_b,
                },
            )
        }
        (None, Some(b)) => {
            let (run_a, run_b) = (open(&args.a)?, open(b)?);
            let (name_a, dir_a, file_a) = pick(&run_a, target, "A")?;
            let (name_b, dir_b, file_b) = pick(&run_b, target, "B")?;
            (
                Side {
                    label: args.a.clone(),
                    dir: dir_a,
                    name: name_a,
                    file: file_a,
                },
                Side {
                    label: b.clone(),
                    dir: dir_b,
                    name: name_b,
                    file: file_b,
                },
            )
        }
        (None, None) => return Err("give two runs to compare, or one run with --against".into()),
    };

    let settings = Settings {
        threshold: args.threshold,
        max_diff_ratio: args.max_diff_ratio,
        out: args.out.clone().into(),
    };
    let report = visual(&side_a.target(), &side_b.target(), &settings).map_err(|e| e.to_string())?;

    let mut text = match args.format {
        Format::Markdown => render_markdown(&report),
        Format::Json => serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?,
    };
    if !text.ends_with('\n') {
        text.push('\n');
    }
    print!("{text}");
    Ok(if args.fail_on_different && report.summary.different > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// The one target of a side: its name, its folder and its components. A run with several targets
/// needs one to be named.
fn pick(
    run: &RunFolder,
    name: Option<&str>,
    side: &str,
) -> Result<(String, std::path::PathBuf, ComponentsFile), String> {
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
            .find(|t| t.dir == name)
            .ok_or_else(|| format!("side {side} has no target {name}; it has: {}", names()))?,
        None => match run.targets.as_slice() {
            [only] => only,
            [] => return Err(format!("side {side} has no targets")),
            _ => {
                return Err(format!(
                    "side {side} has several targets ({}); name one with --target",
                    names()
                ))
            }
        },
    };
    let file = read_components(target).map_err(|e| e.to_string())?;
    Ok((target.dir.clone(), target.path.clone(), file))
}
