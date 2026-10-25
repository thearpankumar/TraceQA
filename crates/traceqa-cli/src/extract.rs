//! `traceqa extract`: arguments in, one run out, a table or JSON printed.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Args;
use traceqa_engine::driver::Drivers;
use traceqa_engine::{driver_commands, EngineError, Execution, Run};
use traceqa_protocol::types::{
    BrowserTarget, Event, ExtractOptions, RunManifest, RunStartParams, RunStatus, StorageState, Viewport,
};

use crate::args::{parse_browser, parse_viewport, DriverArgs};
use crate::interrupt;

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
    interrupt::install(interrupt::Hooks {
        first_message: "
Cancelling: closing the browsers. Press Ctrl-C again to quit at once.",
        waits_for_run_folder: true,
        exits_when_idle: false,
        on_exit: || {},
        first_signal_bound: Some(interrupt::FIRST_SIGNAL_BOUND),
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
