//! `traceqa extract`: arguments in, one run out, a table or JSON printed.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use std::sync::atomic::Ordering;
use std::sync::Once;

use clap::Args;
use traceqa_engine::driver::Drivers;
use traceqa_engine::{driver_commands, EngineError, Execution, Run};
use traceqa_protocol::types::{
    BrowserTarget, Event, ExtractOptions, GroupRun, RunGroup, RunManifest, RunStartParams, RunStatus, StabilityFile,
    StorageState, Viewport,
};
use traceqa_recorder::{new_run_id, now_rfc3339, read_components, TargetRef};

use crate::args::{parse_browser, parse_viewport, DriverArgs};
use crate::interrupt;
use crate::stability::{self, RunSeen, Seen};

#[derive(Args)]
pub struct ExtractArgs {
    /// Page to read.
    url: String,
    /// Browsers to run in, comma separated. Use cdp=ENDPOINT to attach to a running browser,
    /// or webdriver=URL for a WebDriver server or cloud grid. Naming a browser twice runs it
    /// twice; the second folder ends in -2. Names the default drivers do not open are refused.
    #[arg(long, value_delimiter = ',', default_value = "chromium", value_parser = parse_browser)]
    browser: Vec<BrowserTarget>,
    /// Viewport sizes as WIDTHxHEIGHT, comma separated.
    #[arg(long, value_delimiter = ',', default_value = "1280x800", value_parser = parse_viewport)]
    viewport: Vec<Viewport>,
    /// Where to create the run folder.
    #[arg(long, default_value = "runs")]
    out: String,
    /// Show the browser windows.
    #[arg(long)]
    headed: bool,
    /// Also save pictures: a screenshot of the page and one crop per component. Slower,
    /// because the page is then read and photographed one screen at a time.
    #[arg(long)]
    crops: bool,
    /// Leave out elements that are not visible.
    #[arg(long)]
    visible_only: bool,
    /// How many screens to scroll through on a long page. 0 reads the page without scrolling.
    #[arg(long, value_name = "N")]
    max_scroll_steps: Option<u64>,
    /// Time allowed for a page to load, in milliseconds (at least 1).
    #[arg(long, value_name = "MS", value_parser = clap::value_parser!(u64).range(1..))]
    timeout_ms: Option<u64>,
    /// Extra time to wait after the page has loaded, for pages with a timed intro or splash screen.
    #[arg(long, value_name = "MS")]
    settle_ms: Option<u64>,
    /// How many browsers to run at the same time. 1 runs them one after another.
    /// Default: chosen from the machine's cores, at most 4.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    parallel: Option<u64>,
    /// Also print where each browser's time went.
    #[arg(long)]
    timings: bool,
    /// Print the run manifest as JSON instead of a table. Standard output then holds that one
    /// document and nothing else; progress and the run folder go to standard error.
    #[arg(long)]
    json: bool,
    /// Start every browser with the cookies and stored values in FILE, for pages behind a
    /// sign-in. Make one with `traceqa login`. The file is a secret: keep it out of git.
    #[arg(long, value_name = "FILE")]
    storage_state: Option<PathBuf>,
    /// Read the page N times (1 to 20), one full run after another, and report how often each
    /// component was found. Each run is an ordinary run folder; a group folder next to them
    /// (runs/g_ID) holds group.json and stability.json. With --json, standard output holds the
    /// stability.json document only.
    #[arg(long, value_name = "N", default_value_t = 1, value_parser = clap::value_parser!(u64).range(1..=20))]
    repeat: u64,
    /// With --repeat: exit with 1 when any component was not found in every run (or its count,
    /// state or name changed).
    #[arg(long)]
    fail_on_unstable: bool,
    #[command(flatten)]
    drivers: DriverArgs,
}

pub fn run(args: &ExtractArgs) -> ExitCode {
    let request = match build_request(args) {
        Ok(request) => request,
        Err(why) => {
            eprintln!("traceqa: {why}");
            return ExitCode::from(2);
        }
    };
    if args.repeat > 1 {
        return run_repeat(args, &request);
    }
    let (folder, manifest) = match execute_request(request, &args.drivers.drivers, false) {
        Ok(done) => done,
        Err(code) => return code,
    };
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&manifest).expect("spec types always serialise")
        );
        eprintln!("Run folder: {}", folder.display());
    } else {
        print_summary(&manifest);
        if args.timings {
            print_timings(&manifest);
        }
        println!("\nRun folder: {}", folder.display());
    }
    match manifest.status {
        RunStatus::Completed => ExitCode::SUCCESS,
        RunStatus::Cancelled => ExitCode::from(interrupt::INTERRUPTED),
        _ => ExitCode::FAILURE,
    }
}

/// Turns the arguments into the request every client sends. The error is the message for a
/// wrong command (exit code 2); no run folder exists yet.
fn build_request(args: &ExtractArgs) -> Result<RunStartParams, String> {
    let mut targets = args.browser.clone();
    for target in &mut targets {
        target.headless = Some(!args.headed);
    }
    let storage_state = args.storage_state.as_deref().map(read_storage_state).transpose()?;
    // With the default drivers the browser names are known, so a typo fails here, before a run
    // folder exists. A driver the person configures may open other names.
    let custom_drivers = !args.drivers.drivers.is_empty()
        || std::env::var(traceqa_engine::run::DRIVERS_ENV).is_ok_and(|value| !value.trim().is_empty());
    if !custom_drivers {
        if let Some(error) = targets.iter().find_map(|target| target.check_known().err()) {
            return Err(error.to_string());
        }
    }
    Ok(RunStartParams {
        feature: "extract".to_string(),
        url: args.url.clone(),
        targets,
        viewports: args.viewport.clone(),
        out_dir: Some(args.out.clone()),
        parallel: args.parallel,
        options: Some(ExtractOptions {
            max_scroll_steps: args.max_scroll_steps,
            timeout_ms: args.timeout_ms,
            settle_ms: args.settle_ms,
            crops: Some(args.crops),
            include_hidden: Some(!args.visible_only),
        }),
        storage_state,
    })
}

/// Prepares and executes one run with the interrupt hooks installed, so Ctrl-C closes the
/// browsers. Commands that start runs (extract, and later repeat and map) call this and never
/// set up signals themselves. Progress goes to standard error unless `quiet`. The error is the
/// exit code, after the message has been printed.
pub(crate) fn execute_request(
    request: RunStartParams,
    driver_args: &[String],
    quiet: bool,
) -> Result<(PathBuf, RunManifest), ExitCode> {
    let out_dir = request.out_dir.clone().unwrap_or_default();
    let run = match Run::prepare(request) {
        Ok(run) => run,
        Err(EngineError::Io(error)) => {
            // The operating system's text alone ("Access is denied") names neither flag nor path.
            eprintln!("traceqa: --out {out_dir}: {error}");
            return Err(ExitCode::from(2));
        }
        Err(error) => {
            eprintln!("traceqa: {error}");
            return Err(ExitCode::from(2));
        }
    };
    let folder = run.folder().to_path_buf();

    let commands = driver_commands(driver_args);
    // One handler for the whole process: --repeat calls this once per run.
    static HANDLER: Once = Once::new();
    HANDLER.call_once(|| {
        interrupt::install(interrupt::Hooks {
            first_message: "
Cancelling: closing the browsers. Press Ctrl-C again to quit at once.",
            waits_for_run_folder: true,
            exits_when_idle: false,
            on_exit: || {},
            first_signal_bound: Some(interrupt::FIRST_SIGNAL_BOUND),
        });
    });
    let execution = Execution {
        drivers: &|| Drivers::new(commands.clone()),
        cancel: &interrupt::CANCEL,
        policy: None,
    };
    // Browsers run side by side, so each line says which one it is about.
    let mut show_progress = |event: &Event| {
        if quiet {
            return;
        }
        match &event.target {
            Some(target) => interrupt::say(&format!("[{target}] {}", event.message)),
            None => interrupt::say(&event.message),
        }
    };
    let manifest = run.execute_abortable(&execution, &interrupt::ABORT, &mut show_progress);
    Ok((folder, manifest))
}

/// `--repeat N`: N sequential full runs, then a group folder next to them with `group.json` and
/// `stability.json`. A Ctrl-C stops after the current run (the engine cancels it and closes the
/// browsers), and the group is finished with the runs there are.
fn run_repeat(args: &ExtractArgs, request: &RunStartParams) -> ExitCode {
    let total = args.repeat;
    if args.crops {
        eprintln!(
            "traceqa: --repeat {total} with --crops takes a screenshot and every crop {total} times, which is the slowest step."
        );
    }
    let out = Path::new(&args.out);
    let group_id = format!("g_{}", &new_run_id()[2..]);
    let group_dir = out.join(&group_id);
    if let Err(error) = std::fs::create_dir_all(out).and_then(|()| std::fs::create_dir(&group_dir)) {
        eprintln!("traceqa: --out {}: {error}", args.out);
        return ExitCode::from(2);
    }
    let mut group = RunGroup {
        format_version: 1,
        group_id: group_id.clone(),
        kind: "repeat".to_string(),
        url: args.url.clone(),
        runs: Vec::new(),
        started_at: now_rfc3339(),
        finished_at: None,
    };
    if let Err(error) = write_json(&group_dir.join("group.json"), &group) {
        eprintln!("traceqa: cannot write {}: {error}", group_dir.display());
        return ExitCode::from(2);
    }

    let mut folders: Vec<(PathBuf, RunManifest)> = Vec::new();
    let mut refused = None;
    for number in 1..=total {
        if interrupt::CANCEL.load(Ordering::SeqCst) {
            break;
        }
        interrupt::say(&format!("\nRun {number} of {total}"));
        let (folder, manifest) = match execute_request(request.clone(), &args.drivers.drivers, false) {
            Ok(done) => done,
            Err(code) => {
                refused = Some(code);
                break;
            }
        };
        if !args.json {
            println!("\nRun {number} of {total}: {}", folder.display());
            print_summary(&manifest);
            if args.timings {
                print_timings(&manifest);
            }
        }
        group.runs.push(GroupRun {
            run_id: manifest.run_id.clone(),
            status: manifest.status,
        });
        // The index is kept current, so a killed command leaves a group that lists its runs.
        let _ = write_json(&group_dir.join("group.json"), &group);
        let cancelled = manifest.status == RunStatus::Cancelled;
        folders.push((folder, manifest));
        if cancelled {
            break;
        }
    }

    let run_ids: Vec<String> = group.runs.iter().map(|r| r.run_id.clone()).collect();
    let seen: Vec<RunSeen> = folders
        .iter()
        .map(|(folder, manifest)| read_run(folder, manifest))
        .collect();
    let file = stability::compute(&group_id, &args.url, total, &run_ids, &seen);
    group.finished_at = Some(now_rfc3339());
    let written = write_json(&group_dir.join("stability.json"), &file)
        .and_then(|()| write_json(&group_dir.join("group.json"), &group));
    if let Err(error) = written {
        eprintln!("traceqa: cannot write {}: {error}", group_dir.display());
    }

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&file).expect("spec types always serialise")
        );
        for (folder, _) in &folders {
            eprintln!("Run folder: {}", folder.display());
        }
        eprintln!("Group folder: {}", group_dir.display());
    } else {
        print_stability(&file);
        println!("\nGroup folder: {}", group_dir.display());
    }

    let unstable: u64 = file.targets.iter().map(|t| t.unstable_count).sum();
    if interrupt::CANCEL.load(Ordering::SeqCst) || group.runs.iter().any(|r| r.status == RunStatus::Cancelled) {
        ExitCode::from(interrupt::INTERRUPTED)
    } else if let Some(code) = refused {
        code
    } else if group.runs.iter().any(|r| r.status != RunStatus::Completed) || (args.fail_on_unstable && unstable > 0) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// What the completed targets of one run showed. A target whose file cannot be read is left out,
/// as a target that did not complete is: it says nothing about the components.
fn read_run(folder: &Path, manifest: &RunManifest) -> RunSeen {
    let mut seen = RunSeen::new();
    for summary in manifest.targets.iter().filter(|t| t.status == RunStatus::Completed) {
        let target = TargetRef {
            dir: summary.dir.clone(),
            path: folder.join(&summary.dir),
            summary: Some(summary.clone()),
        };
        let Ok(file) = read_components(&target) else {
            eprintln!(
                "traceqa: could not read {}/{}/components.json",
                folder.display(),
                summary.dir
            );
            continue;
        };
        let components = file
            .components
            .iter()
            .map(|c| Seen {
                id: c.id.clone(),
                role: c.role.clone(),
                name: c.name.clone(),
                instances: c.instances.len() as u64,
                visible: c.instances.iter().any(|i| i.state.visible),
                enabled: c.instances.iter().any(|i| i.state.enabled),
            })
            .collect();
        seen.insert(summary.dir.clone(), components);
    }
    seen
}

/// `Stability over N runs: K unstable components` and the ten least steady.
fn print_stability(file: &StabilityFile) {
    let unstable = stability::unstable(file);
    println!(
        "\nStability over {} runs: {} unstable component{}",
        file.runs.len(),
        unstable.len(),
        if unstable.len() == 1 { "" } else { "s" }
    );
    for (dir, component) in unstable.iter().take(10) {
        println!(
            "  {:>3.0}%  {:<8} {:<30} {:<20} {}",
            component.rate * 100.0,
            component.role,
            format!("{:?}", component.name),
            dir,
            component.flags.join(", ")
        );
    }
    if unstable.len() > 10 {
        println!("  ... and {} more in stability.json", unstable.len() - 10);
    }
}

/// Writes pretty JSON through a temporary file, so a reader never sees half a file.
fn write_json(path: &Path, value: &impl serde::Serialize) -> std::io::Result<()> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, text).and_then(|()| std::fs::rename(&temporary, path))
}

/// Reads a state file. It holds session keys, so an error names the file, never what is in it.
fn read_storage_state(path: &Path) -> Result<StorageState, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|_| {
        format!(
            "{} is not a storage state file: expected cookies and origins, as `traceqa login` saves",
            path.display()
        )
    })
}

fn print_summary(manifest: &RunManifest) {
    println!(
        "\n{:<24} {:<10} {:>10} {:>9} {:>8} {:>7}",
        "TARGET", "STATUS", "COMPONENTS", "INSTANCES", "WARNINGS", "TIME"
    );
    for target in &manifest.targets {
        println!(
            "{:<24} {:<10} {:>10} {:>9} {:>8} {:>6.1}s",
            target.dir,
            target.status.to_string(),
            target.component_count,
            target.instance_count,
            target.warning_count,
            target.duration_ms as f64 / 1000.0
        );
        if let Some(error) = &target.error {
            println!("  {error}");
        }
    }
}

/// Where each target's time went, in seconds.
fn print_timings(manifest: &RunManifest) {
    println!(
        "\n{:<24} {:>7} {:>6} {:>6} {:>6} {:>7} {:>6} {:>6}",
        "TARGET", "SCREENS", "OPEN", "LOAD", "READ", "SCROLL", "SHOTS", "CROPS"
    );
    for target in &manifest.targets {
        let Some(timings) = &target.timings else { continue };
        let seconds = |ms: u64| ms as f64 / 1000.0;
        println!(
            "{:<24} {:>7} {:>5.1}s {:>5.1}s {:>5.1}s {:>6.1}s {:>5.1}s {:>5.1}s",
            target.dir,
            timings.screens.unwrap_or(0),
            seconds(timings.open_ms),
            seconds(timings.load_ms),
            seconds(timings.read_ms),
            seconds(timings.scroll_ms),
            seconds(timings.screenshot_ms),
            seconds(timings.crop_ms),
        );
    }
}
