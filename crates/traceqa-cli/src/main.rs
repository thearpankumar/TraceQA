//! The `traceqa` command. Thin on purpose: it turns arguments into the same
//! request every other client sends, and hands it to the engine.

mod install;

use std::io::{self, BufReader};
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

use clap::{Args, Parser, Subcommand};
use traceqa_engine::driver::Drivers;
use traceqa_engine::{driver_commands, serve, Execution, Run};
use traceqa_protocol::types::{BrowserTarget, Event, ExtractOptions, RunManifest, RunStartParams, RunStatus, Viewport};

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
    Serve(DriverArgs),
    /// Let AI tools such as Claude Code run TraceQA (an MCP server).
    ///
    /// Speaks the Model Context Protocol on stdin and stdout. Add it to Claude Code with:
    /// claude mcp add traceqa -- traceqa mcp
    Mcp(McpArgs),
    /// Install the browser drivers and browsers into TraceQA's own folder (~/.traceqa).
    Install(install::InstallArgs),
    /// Check the setup: where the drivers are, whether they start, and which browsers they open.
    Doctor(DriverArgs),
}

#[derive(Args)]
struct McpArgs {
    /// Where runs are written and read.
    #[arg(long, default_value = "runs")]
    out: String,
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
    /// or webdriver=URL for a WebDriver server or cloud grid.
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
    /// Time allowed for a page to load, in milliseconds.
    #[arg(long, value_name = "MS")]
    timeout_ms: Option<u64>,
    /// Extra time to wait after the page has loaded, for pages with a timed intro or splash screen.
    #[arg(long, value_name = "MS")]
    settle_ms: Option<u64>,
    /// How many browsers to run at the same time. 1 runs them one after another.
    /// Default: chosen from the machine's cores, at most 4.
    #[arg(long, value_name = "N")]
    parallel: Option<u64>,
    /// Also print where each browser's time went.
    #[arg(long)]
    timings: bool,
    /// Print the run manifest as JSON instead of a table.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    drivers: DriverArgs,
}

fn parse_viewport(text: &str) -> Result<Viewport, String> {
    Viewport::parse(text)
}

fn parse_browser(text: &str) -> Result<BrowserTarget, String> {
    BrowserTarget::parse(text)
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Extract(args) => extract(args),
        Command::Serve(args) => {
            let stdin = BufReader::new(io::stdin());
            match serve::serve(stdin, Box::new(io::stdout()), driver_commands(&args.drivers)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("traceqa: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::Mcp(args) => mcp(args),
        Command::Install(args) => install::install(args),
        Command::Doctor(args) => install::doctor(&args.drivers),
    }
}

fn mcp(args: McpArgs) -> ExitCode {
    let runs_dir = match std::path::absolute(Path::new(&args.out)) {
        Ok(dir) => dir,
        Err(error) => {
            eprintln!("traceqa: {}: {error}", args.out);
            return ExitCode::from(2);
        }
    };
    let config = traceqa_mcp::Config {
        runs_dir,
        drivers: driver_commands(&args.drivers.drivers),
    };
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
    };
    let run = match Run::prepare(request) {
        Ok(run) => run,
        Err(error) => {
            eprintln!("traceqa: {error}");
            return ExitCode::from(2);
        }
    };
    let folder = run.folder().to_path_buf();

    let commands = driver_commands(&args.drivers.drivers);
    let execution = Execution {
        drivers: &|| Drivers::new(commands.clone()),
        cancel: &AtomicBool::new(false),
    };
    // Browsers run side by side, so each line says which one it is about.
    let mut show_progress = |event: &Event| match &event.target {
        Some(target) => eprintln!("[{target}] {}", event.message),
        None => eprintln!("{}", event.message),
    };
    let manifest = run.execute(&execution, &mut show_progress);

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&manifest).expect("spec types always serialise")
        );
    } else {
        print_summary(&manifest);
        if args.timings {
            print_timings(&manifest);
        }
        println!("\nRun folder: {}", folder.display());
    }
    if manifest.status == RunStatus::Completed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
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
