//! The `traceqa` command. Thin on purpose: it turns arguments into the same
//! request every other client sends, and hands it to the engine.

mod install;
mod interrupt;
mod login;

use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use traceqa_engine::driver::Drivers;
use traceqa_engine::{driver_commands, serve, EngineError, Execution, Run};
use traceqa_protocol::types::{
    BrowserTarget, Event, ExtractOptions, RunManifest, RunStartParams, RunStatus, StorageState, Viewport,
};
use traceqa_protocol::ParseError;

#[derive(Parser)]
#[command(
    name = "traceqa",
    version,
    about = "Find every interactive component on a page, in any browser."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List every interactive component on a page.
    Extract(ExtractArgs),
    /// Speak the client protocol on stdin and stdout. This is what SDKs start.
    Serve(ServeArgs),
    /// Let AI tools such as Claude Code run TraceQA (an MCP server).
    ///
    /// Speaks the Model Context Protocol on stdin and stdout. Add it to Claude Code with:
    /// claude mcp add traceqa -- traceqa mcp
    Mcp(McpArgs),
    /// Install the browser drivers and browsers into TraceQA's own folder (~/.traceqa).
    Install(install::InstallArgs),
    /// Check the setup: where the drivers are, whether they start, and which browsers they open.
    Doctor(DoctorArgs),
    /// Sign in once in a browser window and save its state, for pages behind a sign-in.
    ///
    /// Then run: traceqa extract URL --storage-state FILE
    Login(login::LoginArgs),
}

#[derive(Args)]
struct McpArgs {
    /// Where runs are written and read.
    #[arg(long, default_value = "runs")]
    out: String,
    /// A folder of saved sign-in states (see `traceqa login --save`). With it, the AI can name
    /// a file in this folder as `storage_state`; without it, the AI cannot use sign-ins at all.
    #[arg(long, value_name = "DIR")]
    state_dir: Option<PathBuf>,
    /// Let the AI attach to a running browser (cdp=ENDPOINT) or a WebDriver server
    /// (webdriver=URL). Off by default: those name a machine for this server to connect to.
    #[arg(long)]
    allow_remote_browsers: bool,
    /// Refuse pages on loopback, private (RFC 1918), shared (100.64/10), unique-local and
    /// multicast addresses, also when a redirect leads there. Cloud metadata and link-local
    /// addresses are always refused, with or without this flag.
    #[arg(long)]
    block_private: bool,
    /// With --block-private, still allow this host (a name or an address). Repeat for several.
    /// Never lifts the ban on metadata and link-local addresses.
    #[arg(long = "allow-host", value_name = "HOST")]
    allow_hosts: Vec<String>,
    /// How many extractions may run at once. More are answered "busy" at once, so a host that
    /// fires many calls cannot start many browsers. Reading tools have a higher limit of their own.
    #[arg(long, value_name = "N", default_value_t = traceqa_mcp::DEFAULT_MAX_CONCURRENT_EXTRACTS)]
    max_concurrent_extracts: usize,
    #[command(flatten)]
    drivers: DriverArgs,
}

#[derive(Args)]
struct DoctorArgs {
    /// Also show the drivers' own log lines (on standard error).
    #[arg(long)]
    verbose: bool,
    #[command(flatten)]
    drivers: DriverArgs,
}

#[derive(Args)]
struct ServeArgs {
    /// How many runs may be going at once. A run.start beyond this is refused with a "busy"
    /// error (retryable) instead of starting more browsers.
    #[arg(long, value_name = "N", default_value_t = serve::DEFAULT_MAX_RUNS)]
    max_runs: usize,
    #[command(flatten)]
    drivers: DriverArgs,
}

#[derive(Args)]
struct DriverArgs {
    /// Command that starts a browser driver. Repeat for several. Default: $TRACEQA_DRIVERS,
    /// then traceqa-driver-playwright and traceqa-driver-webdriver from PATH.
    #[arg(long = "driver", value_name = "COMMAND")]
    drivers: Vec<String>,
}

#[derive(Args)]
struct ExtractArgs {
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

fn parse_viewport(text: &str) -> Result<Viewport, ParseError> {
    Viewport::parse(text)
}

fn parse_browser(text: &str) -> Result<BrowserTarget, ParseError> {
    BrowserTarget::parse(text)
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Extract(args) => extract(args),
        Command::Serve(args) => {
            install_server_interrupt("traceqa: stopping: cancelling the runs. Press Ctrl-C again to quit at once.");
            let stdin = BufReader::new(io::stdin());
            let options = serve::ServeOptions {
                max_runs: args.max_runs,
                interrupt: Some(server_interrupt()),
            };
            match serve::serve_with(
                stdin,
                Box::new(io::stdout()),
                driver_commands(&args.drivers.drivers),
                &options,
            ) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("traceqa: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::Mcp(args) => mcp(&args),
        Command::Install(args) => install::install(&args),
        Command::Doctor(args) => install::doctor(&args.drivers.drivers, args.verbose),
        Command::Login(args) => login::login(&args),
    }
}

/// What `serve` and `mcp` share with the signal handler: the cancel flag and the abort request.
fn server_interrupt() -> traceqa_engine::Interrupt {
    traceqa_engine::Interrupt {
        cancel: &interrupt::CANCEL,
        abort: &interrupt::ABORT,
    }
}

/// Ctrl-C, SIGTERM and SIGHUP for a server: the first cancels every run and the process leaves
/// when none is in flight; a second one, or the bound, quits at once. Stdout carries the
/// protocol, so everything said here goes to stderr.
fn install_server_interrupt(first_message: &'static str) {
    interrupt::install(interrupt::Hooks {
        first_message,
        waits_for_run_folder: true,
        exits_when_idle: true,
        on_exit: || {},
        first_signal_bound: Some(interrupt::FIRST_SIGNAL_BOUND),
    });
}

fn mcp(args: &McpArgs) -> ExitCode {
    let runs_dir = match std::path::absolute(Path::new(&args.out)) {
        Ok(dir) => dir,
        Err(error) => {
            eprintln!("traceqa: {}: {error}", args.out);
            return ExitCode::from(2);
        }
    };
    let state_dir = match args.state_dir.as_deref().map(std::fs::canonicalize).transpose() {
        Ok(dir) if dir.as_ref().is_none_or(|dir| dir.is_dir()) => dir,
        _ => {
            eprintln!(
                "traceqa: --state-dir {} is not a folder",
                args.state_dir.as_deref().unwrap_or(Path::new("")).display()
            );
            return ExitCode::from(2);
        }
    };
    let config = traceqa_mcp::Config {
        runs_dir,
        drivers: driver_commands(&args.drivers.drivers),
        state_dir,
        allow_remote_browsers: args.allow_remote_browsers,
        policy: traceqa_mcp::UrlPolicy::new(args.block_private, &args.allow_hosts),
        max_concurrent_extracts: args.max_concurrent_extracts,
        interrupt: Some(server_interrupt()),
    };
    install_server_interrupt("traceqa: stopping: cancelling the calls. Press Ctrl-C again to quit at once.");
    // Stdout belongs to the protocol. Everything for people goes to stderr, which hosts log.
    match traceqa_mcp::serve(BufReader::new(io::stdin()), Box::new(io::stdout()), config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("traceqa: {error}");
            ExitCode::FAILURE
        }
    }
}

fn extract(args: ExtractArgs) -> ExitCode {
    let mut targets = args.browser;
    for target in &mut targets {
        target.headless = Some(!args.headed);
    }
    let storage_state = match args.storage_state.as_deref().map(read_storage_state).transpose() {
        Ok(state) => state,
        Err(why) => {
            eprintln!("traceqa: {why}");
            return ExitCode::from(2);
        }
    };
    // With the default drivers the browser names are known, so a typo fails here, before a run
    // folder exists. A driver the person configures may open other names.
    let custom_drivers = !args.drivers.drivers.is_empty()
        || std::env::var(traceqa_engine::run::DRIVERS_ENV).is_ok_and(|value| !value.trim().is_empty());
    if !custom_drivers {
        if let Some(error) = targets.iter().find_map(|target| target.check_known().err()) {
            eprintln!("traceqa: {error}");
            return ExitCode::from(2);
        }
    }
    let out_dir = args.out.clone();
    let request = RunStartParams {
        feature: "extract".to_string(),
        url: args.url,
        targets,
        viewports: args.viewport,
        out_dir: Some(args.out),
        parallel: args.parallel,
        options: Some(ExtractOptions {
            max_scroll_steps: args.max_scroll_steps,
            timeout_ms: args.timeout_ms,
            settle_ms: args.settle_ms,
            crops: Some(args.crops),
            include_hidden: Some(!args.visible_only),
        }),
        storage_state,
    };
    let run = match Run::prepare(request) {
        Ok(run) => run,
        Err(EngineError::Io(error)) => {
            // The operating system's text alone ("Access is denied") names neither flag nor path.
            eprintln!("traceqa: --out {out_dir}: {error}");
            return ExitCode::from(2);
        }
        Err(error) => {
            eprintln!("traceqa: {error}");
            return ExitCode::from(2);
        }
    };
    let folder = run.folder().to_path_buf();

    let commands = driver_commands(&args.drivers.drivers);
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
    let mut show_progress = |event: &Event| match &event.target {
        Some(target) => interrupt::say(&format!("[{target}] {}", event.message)),
        None => interrupt::say(&event.message),
    };
    let manifest = run.execute_abortable(&execution, &interrupt::ABORT, &mut show_progress);

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
