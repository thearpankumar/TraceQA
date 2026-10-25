//! `traceqa findings`: automated checks over a finished run, printed and written next to each
//! target's `components.json` as `findings.json`.
//!
//! The checks themselves are in the `traceqa-findings` crate. This file reads the run folder,
//! applies the suppression file, prints, and writes. Only this command writes `findings.json`;
//! `extract` does not, so a run folder is the same with or without it.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Args, ValueEnum};
use traceqa_findings::{analyse_run, severity_rank, Options, RunInput, Suppressions, TargetInput};
use traceqa_protocol::types::{ComponentsFile, FindingScope, FindingSeverity, FindingsFile, RunManifest, RunStatus};

/// Where the suppression file is looked for when `--suppressions` is not given.
const DEFAULT_SUPPRESSIONS: &str = ".traceqa/suppressions.yaml";

#[derive(Clone, Copy, ValueEnum)]
enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    fn into_protocol(self) -> FindingSeverity {
        match self {
            Self::Info => FindingSeverity::Info,
            Self::Warning => FindingSeverity::Warning,
            Self::Error => FindingSeverity::Error,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum FailOn {
    Error,
    Warning,
    Info,
    Never,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Table,
    Json,
    Markdown,
    /// GitHub Actions workflow commands; also fills $GITHUB_STEP_SUMMARY.
    Github,
    /// SARIF 2.1.0, for SARIF viewers and other tools (not GitHub code scanning: see docs/usage.md).
    Sarif,
}

#[derive(Args)]
pub struct FindingsArgs {
    /// Run folder made by `traceqa extract`.
    run: PathBuf,
    /// Only this target, for example firefox-1280x800. Default: all.
    #[arg(long, value_name = "NAME")]
    target: Option<String>,
    /// Suppression file (YAML). Default: .traceqa/suppressions.yaml when it exists.
    #[arg(long, value_name = "FILE")]
    suppressions: Option<PathBuf>,
    /// Leave out findings below this severity.
    #[arg(long, value_enum, default_value = "info")]
    min_severity: Severity,
    /// Exit with 1 when a finding of this severity or worse is left.
    #[arg(long, value_enum, default_value = "never")]
    fail_on: FailOn,
    /// How to print.
    #[arg(long, value_enum, default_value = "table")]
    format: Format,
    /// Only print; do not write findings.json.
    #[arg(long)]
    no_write: bool,
    /// Today as YYYY-MM-DD, for suppression expiry. Tests only.
    #[arg(long, hide = true, value_name = "DATE")]
    today: Option<String>,
}

pub fn run(args: &FindingsArgs) -> ExitCode {
    match execute(args) {
        Ok(code) => code,
        Err(why) => {
            eprintln!("traceqa: {why}");
            ExitCode::from(2)
        }
    }
}

fn execute(args: &FindingsArgs) -> Result<ExitCode, String> {
    let options = Options {
        min_severity: Some(args.min_severity.into_protocol()),
        suppressions: read_suppressions(args.suppressions.as_deref())?,
        today: Some(match &args.today {
            Some(today) if traceqa_findings::suppress::is_date(today) => today.clone(),
            Some(today) => return Err(format!("--today {today}: expected YYYY-MM-DD")),
            None => today(),
        }),
    };
    let run = read_run(&args.run)?;
    if let Some(wanted) = &args.target {
        if !run.targets.iter().any(|t| &t.dir == wanted) {
            let known: Vec<&str> = run.targets.iter().map(|t| t.dir.as_str()).collect();
            return Err(format!("--target {wanted}: the run has {}", known.join(", ")));
        }
    }
    // Every target is analysed so a suppression used in another browser is not reported unused;
    // only the chosen one is printed and written.
    let files = analyse_run(&run, &options);
    let shown: Vec<&FindingsFile> = files
        .iter()
        .filter(|f| match (&args.target, &f.target) {
            (Some(wanted), Some(dir)) => wanted == dir,
            (Some(_), None) => false,
            (None, _) => true,
        })
        .collect();

    if !args.no_write {
        for file in &shown {
            write_file(&args.run, file)?;
        }
        if args.target.is_none() && !files.iter().any(|f| f.scope == FindingScope::Run) {
            // A run-level file from an earlier run of this command no longer applies.
            let stale = args.run.join("findings.json");
            if stale.is_file() {
                std::fs::remove_file(&stale).map_err(|e| format!("could not remove {}: {e}", stale.display()))?;
            }
        }
    }

    match args.format {
        Format::Json => println!(
            "{}",
            serde_json::to_string_pretty(&shown).expect("spec types always serialise")
        ),
        Format::Table => print!("{}", table(&shown)),
        Format::Markdown => print!("{}", markdown(&shown)),
        Format::Github => {
            print!(
                "{}",
                traceqa_diff::github::render(&traceqa_diff::github::finding_annotations(&shown))
            );
            append_step_summary(&markdown(&shown));
        }
        Format::Sarif => println!(
            "{}",
            serde_json::to_string_pretty(&traceqa_diff::sarif::findings_log(&shown)).expect("JSON always serialises")
        ),
    }
    for file in &shown {
        for warning in &file.warnings {
            eprintln!("traceqa: warning: {}", warning.message);
        }
    }

    let threshold = match args.fail_on {
        FailOn::Error => Some(FindingSeverity::Error),
        FailOn::Warning => Some(FindingSeverity::Warning),
        FailOn::Info => Some(FindingSeverity::Info),
        FailOn::Never => None,
    };
    let failing = threshold.is_some_and(|threshold| {
        shown
            .iter()
            .flat_map(|f| &f.findings)
            .any(|f| severity_rank(f.severity) >= severity_rank(threshold))
    });
    Ok(if failing { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}

fn read_suppressions(explicit: Option<&Path>) -> Result<Option<Suppressions>, String> {
    let path = match explicit {
        Some(path) => path.to_path_buf(),
        None => {
            let default = PathBuf::from(DEFAULT_SUPPRESSIONS);
            if !default.is_file() {
                return Ok(None);
            }
            default
        }
    };
    let size = std::fs::metadata(&path)
        .map_err(|e| format!("could not read {}: {e}", path.display()))?
        .len();
    if size > traceqa_findings::suppress::MAX_FILE_BYTES as u64 {
        return Err(format!("{}: the suppression file is too large", path.display()));
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    traceqa_findings::suppress::parse(&text)
        .map(Some)
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn read_run(folder: &Path) -> Result<RunInput, String> {
    let manifest_path = folder.join("run.json");
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("{} is not a run folder: could not read run.json: {e}", folder.display()))?;
    let manifest: RunManifest =
        serde_json::from_str(&text).map_err(|e| format!("{}: not a run manifest: {e}", manifest_path.display()))?;
    let mut targets = Vec::new();
    for target in &manifest.targets {
        // run.json is data: a folder name must be one plain name inside the run folder.
        if target.dir.is_empty() || target.dir.contains(['/', '\\']) || target.dir == ".." || target.dir == "." {
            return Err(format!(
                "run.json names a target folder that is not a plain name: {:?}",
                target.dir
            ));
        }
        if target.status != RunStatus::Completed {
            eprintln!("traceqa: skipping {}: its status is {}", target.dir, target.status);
            continue;
        }
        let path = folder.join(&target.dir).join("components.json");
        let text = std::fs::read_to_string(&path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
        let file: ComponentsFile =
            serde_json::from_str(&text).map_err(|e| format!("{}: not a components file: {e}", path.display()))?;
        // axe.json exists only when the run was made with --axe.
        let axe = traceqa_recorder::read_axe(&traceqa_recorder::TargetRef {
            dir: target.dir.clone(),
            path: folder.join(&target.dir),
            summary: None,
        })
        .map_err(|e| e.to_string())?;
        targets.push(TargetInput {
            dir: target.dir.clone(),
            file,
            axe,
        });
    }
    Ok(RunInput {
        run_id: manifest.run_id,
        targets,
    })
}

/// Writes `findings.json` for a target, or for the run, by writing a temporary file and renaming
/// it. A file that would not change is left alone, so running twice is byte-identical.
fn write_file(folder: &Path, file: &FindingsFile) -> Result<(), String> {
    let directory = match &file.target {
        Some(dir) => folder.join(dir),
        None => folder.to_path_buf(),
    };
    let path = directory.join("findings.json");
    let mut text = serde_json::to_string_pretty(file).expect("spec types always serialise");
    text.push('\n');
    if std::fs::read_to_string(&path).is_ok_and(|old| old == text) {
        return Ok(());
    }
    let temporary = directory.join(".findings.json.tmp");
    std::fs::write(&temporary, &text)
        .and_then(|()| std::fs::rename(&temporary, &path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&temporary);
            format!("could not write {}: {e}", path.display())
        })
}

fn label(severity: FindingSeverity) -> &'static str {
    match severity {
        FindingSeverity::Error => "error",
        FindingSeverity::Warning => "warning",
        FindingSeverity::Info => "info",
    }
}

fn heading(file: &FindingsFile) -> String {
    match &file.target {
        Some(dir) => dir.clone(),
        None => "run".to_string(),
    }
}

fn table(files: &[&FindingsFile]) -> String {
    let mut out = String::new();
    for file in files {
        let _ = writeln!(
            out,
            "\n{}: {} errors, {} warnings, {} info{}",
            heading(file),
            file.counts.error,
            file.counts.warning,
            file.counts.info,
            if file.suppressed.is_empty() {
                String::new()
            } else {
                format!(", {} suppressed", file.suppressed.len())
            }
        );
        if let (Some(coverage), Some(review)) = (&file.coverage, file.needs_review) {
            let _ = writeln!(out, "axe-core: {review} results need review by a person. {coverage}");
        }
        if !file.findings.is_empty() {
            let _ = writeln!(
                out,
                "{:<8} {:<22} {:<15} {:<10} {:<24} FINDING",
                "SEVERITY", "RULE", "COMPONENT", "ROLE", "NAME"
            );
        }
        for f in &file.findings {
            let _ = writeln!(
                out,
                "{:<8} {:<22} {:<15} {:<10} {:<24} {}",
                label(f.severity),
                f.rule,
                f.component_id,
                f.role,
                shorten(&f.name, 24),
                f.message
                    .trim_end_matches(" This is an automated check, not a conformance claim.")
            );
        }
    }
    out.push_str("\nThese are automated checks, not a conformance claim.\n");
    out
}

fn markdown(files: &[&FindingsFile]) -> String {
    let mut out = String::from("# TraceQA findings\n\nAutomated checks, not a conformance claim.\n");
    for file in files {
        let _ = writeln!(
            out,
            "\n## {}\n\n{} errors, {} warnings, {} info\n",
            heading(file),
            file.counts.error,
            file.counts.warning,
            file.counts.info
        );
        if let (Some(coverage), Some(review)) = (&file.coverage, file.needs_review) {
            let _ = writeln!(
                out,
                "axe-core: {review} results need review by a person. {}\n",
                cell(coverage)
            );
        }
        if !file.findings.is_empty() {
            out.push_str("| Severity | Rule | Component | Name | WCAG | Finding |\n|---|---|---|---|---|---|\n");
        }
        for f in &file.findings {
            let _ = writeln!(
                out,
                "| {} | {} | `{}` | {} | {} | {} |",
                label(f.severity),
                f.rule,
                f.component_id,
                cell(&f.name),
                f.wcag.join(", "),
                cell(&f.message)
            );
        }
        for w in &file.warnings {
            let _ = writeln!(out, "\n> {}", cell(&w.message));
        }
    }
    out
}

/// GitHub Actions names a file whose Markdown becomes the step's summary.
fn append_step_summary(text: &str) {
    use std::io::Write as _;
    let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY").filter(|p| !p.is_empty()) else {
        return;
    };
    let written = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(text.as_bytes()));
    if let Err(e) = written {
        eprintln!("traceqa: warning: could not write $GITHUB_STEP_SUMMARY: {e}");
    }
}

/// Keeps a markdown table cell on one line and stops page text from adding markup.
fn cell(text: &str) -> String {
    text.replace('|', "\\|")
        .replace(['<', '>', '`', '*', '_', '[', ']'], "")
}

fn shorten(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        text.to_string()
    } else {
        let mut short: String = text.chars().take(width - 1).collect();
        short.push('…');
        short
    }
}

/// Today in UTC as YYYY-MM-DD.
fn today() -> String {
    let seconds = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    civil_date((seconds / 86_400) as i64)
}

/// Days since 1970-01-01 to a date (Howard Hinnant's algorithm).
fn civil_date(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(19_782), "2024-02-29");
        assert_eq!(civil_date(20_743), "2026-10-17");
        assert!(traceqa_findings::suppress::is_date(&today()));
    }

    #[test]
    fn markdown_cells_cannot_break_the_table() {
        assert_eq!(cell("a|b <script> `x`"), "a\\|b script x");
    }
}
