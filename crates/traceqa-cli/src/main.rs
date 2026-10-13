//! The `traceqa` command. Thin on purpose: it turns arguments into the same
//! request every other client sends, and hands it to the engine.

use std::io::{self, BufReader};
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
    let size = text
        .split_once('x')
        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
    match size {
        Some((width, height)) if width > 0 && height > 0 => Ok(Viewport { width, height }),
        _ => Err(format!("expected WIDTHxHEIGHT such as 1280x800, got {text:?}")),
    }
}

fn parse_browser(text: &str) -> Result<BrowserTarget, String> {
    let (browser, endpoint) = match text.split_once('=') {
        Some((browser, endpoint)) => (browser, Some(endpoint.to_string())),
        None => (text, None),
    };
    if browser.is_empty() || endpoint.as_deref() == Some("") {
        return Err(format!(
            "expected a browser name, cdp=ENDPOINT or webdriver=URL, got {text:?}"
        ));
    }
    let mut target = BrowserTarget::named(browser);
    if browser == "webdriver" {
        target.webdriver_url = endpoint;
    } else {
        target.cdp_endpoint = endpoint;
    }
    Ok(target)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_needs_two_positive_numbers() {
        assert_eq!(
            parse_viewport("390x844"),
            Ok(Viewport {
                width: 390,
                height: 844
            })
        );
        for bad in ["1280", "0x800", "ax b", "1280x", ""] {
            assert!(parse_viewport(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn browser_may_carry_a_cdp_endpoint() {
        assert_eq!(parse_browser("firefox").unwrap().cdp_endpoint, None);
        let attached = parse_browser("cdp=ws://127.0.0.1:9222/devtools/browser/abc?x=1").unwrap();
        assert_eq!(attached.browser, "cdp");
        assert_eq!(
            attached.cdp_endpoint.as_deref(),
            Some("ws://127.0.0.1:9222/devtools/browser/abc?x=1")
        );
        let remote = parse_browser("webdriver=http://127.0.0.1:9515").unwrap();
        assert_eq!(
            (remote.cdp_endpoint, remote.webdriver_url.as_deref()),
            (None, Some("http://127.0.0.1:9515"))
        );
        assert!(parse_browser("cdp=").is_err() && parse_browser("=ws://x").is_err());
    }
}
