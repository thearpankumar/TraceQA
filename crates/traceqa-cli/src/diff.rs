//! `traceqa diff A B`: what differs between two finished runs. Reads only; never starts a browser.

use std::fs;
use std::process::ExitCode;

use clap::{Args, ValueEnum};
use traceqa_diff::{compare, render_markdown, Options};
use traceqa_protocol::types::ComponentsFile;
use traceqa_recorder::{open_run, read_components, RunFolder};

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Markdown,
    Json,
}

#[derive(Args)]
pub struct DiffArgs {
    /// Side A: a run folder, a target folder, a components.json, or the id of a run in --runs-dir.
    a: String,
    /// Side B, in the same forms. In the report, "added" means in B only.
    b: String,
    /// The target folder to use on both sides when a run holds several (such as chromium-1280x800).
    #[arg(long, value_name = "NAME", conflicts_with_all = ["target_a", "target_b"])]
    target: Option<String>,
    /// The target folder to use on side A.
    #[arg(long, value_name = "NAME")]
    target_a: Option<String>,
    /// The target folder to use on side B.
    #[arg(long, value_name = "NAME")]
    target_b: Option<String>,
    /// Report format. Markdown is for people and pull requests; json is the DiffFile contract.
    #[arg(long, value_enum, default_value = "markdown")]
    format: Format,
    /// Write the report to this file instead of standard output.
    #[arg(long, value_name = "FILE")]
    out: Option<String>,
    /// Where bare run ids are looked up.
    #[arg(long, value_name = "DIR", default_value = "runs")]
    runs_dir: String,
}

/// 0 when the diff ran, whatever it found; 2 for input that cannot be compared. Exit 1 is kept
/// for gates that fail on a difference.
pub fn run(args: &DiffArgs) -> ExitCode {
    match diff(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("traceqa: diff: {message}");
            ExitCode::from(2)
        }
    }
}

fn diff(args: &DiffArgs) -> Result<(), String> {
    let runs_dir = std::path::Path::new(&args.runs_dir);
    let open = |arg: &str| open_run(arg, runs_dir).map_err(|e| e.to_string());
    let (run_a, run_b) = (open(&args.a)?, open(&args.b)?);
    let name_a = args.target_a.as_ref().or(args.target.as_ref());
    let name_b = args.target_b.as_ref().or(args.target.as_ref());
    let (dir_a, file_a) = pick(&run_a, name_a, "A")?;
    let (dir_b, file_b) = pick(&run_b, name_b, "B")?;

    let options = Options {
        label_a: args.a.clone(),
        label_b: args.b.clone(),
        target_a: Some(dir_a),
        target_b: Some(dir_b),
    };
    let result = compare(&file_a, &file_b, &options).map_err(|e| e.to_string())?;
    let mut text = match args.format {
        Format::Markdown => render_markdown(&result),
        Format::Json => serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?,
    };
    if !text.ends_with('\n') {
        text.push('\n');
    }
    match &args.out {
        Some(path) => fs::write(path, text).map_err(|e| format!("--out {path}: {e}"))?,
        None => print!("{text}"),
    }
    Ok(())
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
