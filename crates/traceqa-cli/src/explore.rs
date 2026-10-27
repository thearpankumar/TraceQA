//! `traceqa explore`: walk a site by acting on it, and write down the screens and the actions between them.
//!
//! The walk is in `traceqa_engine::explore`. This file turns arguments into its request, refuses what must be refused
//! before a folder or a browser exists (exit 2), and installs the signal hooks.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Once;
use std::time::Duration;

use clap::{Args, ValueEnum};
use traceqa_common::types::{BrowserTarget, Event, RunStatus, StorageState, Viewport};
use traceqa_engine::driver_commands;
use traceqa_engine::explore::{Budgets, ExploreRun, Options, Request, Stop, Strategy, DEFAULT_CHROME_PAGES};
use traceqa_engine::policy::{check_start, Rules};
use traceqa_engine::state::{Similarity, DEFAULT_SIMILARITY};

use crate::args::{parse_browser, parse_viewport, DriverArgs};
use crate::interrupt;

#[derive(Clone, Copy, ValueEnum)]
enum StrategyArg {
    Bfs,
    Dfs,
}

#[derive(Args)]
pub struct ExploreArgs {
    /// Page to start from. Only its own origin is explored, unless --allow-origin names others.
    url: String,
    /// Browser to explore in. One per run: run the command again for another.
    #[arg(long, default_value = "chromium", value_parser = parse_browser)]
    browser: BrowserTarget,
    /// Viewport size as WIDTHxHEIGHT.
    #[arg(long, default_value = "1280x800", value_parser = parse_viewport)]
    viewport: Viewport,
    /// Where to create the run folder.
    #[arg(long, default_value = "runs")]
    out: String,
    /// Show the browser window.
    #[arg(long)]
    headed: bool,
    /// Start with the cookies and stored values in FILE, for pages behind a sign-in. Make one with
    /// `traceqa login`. The file is a secret: keep it out of git.
    #[arg(long, value_name = "FILE")]
    storage_state: Option<PathBuf>,
    /// Which screen to explore next: `bfs` takes everything one action away before going deeper, `dfs` goes as deep
    /// as it can first.
    #[arg(long, value_enum, default_value = "bfs")]
    strategy: StrategyArg,
    /// Decides the text typed into fields. The same seed on the same site gives the same walk.
    #[arg(long, default_value_t = 0)]
    seed: u64,
    /// How alike two readings of a page must be to count as the same screen: the share of their component ids that
    /// are the same, from 0 to 1.
    #[arg(long, value_name = "SHARE", default_value_t = 0.9)]
    state_similarity: f64,
    /// Most screens to find.
    #[arg(long, value_name = "N", default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=10_000))]
    max_states: u32,
    /// Most actions to make.
    #[arg(long, value_name = "N", default_value_t = 300, value_parser = clap::value_parser!(u32).range(1..=100_000))]
    max_actions: u32,
    /// Most actions from the start page to a screen. Screens deeper than this are found but not explored.
    #[arg(long, value_name = "N", default_value_t = 6, value_parser = clap::value_parser!(u32).range(0..=100))]
    max_depth: u32,
    /// The most one action may take, in seconds.
    #[arg(long, value_name = "SECONDS", default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..=300))]
    action_timeout: u64,
    /// The most the whole walk may take, in seconds.
    #[arg(long, value_name = "SECONDS", default_value_t = 600, value_parser = clap::value_parser!(u64).range(1..=86_400))]
    timeout: u64,
    /// Stop when the browser has sent this many requests to one origin.
    #[arg(long, value_name = "N", default_value_t = 2000, value_parser = clap::value_parser!(u64).range(1..))]
    max_requests_per_origin: u64,
    /// Pages to read before the walk, besides the first, to find what is on every page (header, footer). They are
    /// links the safety rules allow, opened and never clicked. 0 reads only the first.
    #[arg(long, value_name = "N", default_value_t = DEFAULT_CHROME_PAGES, value_parser = clap::value_parser!(usize))]
    chrome_pages: usize,
    /// Let the walk type into fields and press buttons that are not links, tabs or open-and-close controls. Without
    /// it the browser refuses every request that is not GET or HEAD. Needs --allow-origin for this site.
    #[arg(long)]
    allow_writes: bool,
    /// An origin the walk may go to (scheme, host and port). Repeat for several. With --allow-writes, the start
    /// site must be one of them.
    #[arg(long, value_name = "ORIGIN")]
    allow_origin: Vec<String>,
    /// A word or phrase that marks a control as not to be touched, in any mode. Added to the built-in lists of
    /// delete, pay, send, sign out and the like. Repeat for several.
    #[arg(long, value_name = "WORD")]
    deny_name: Vec<String>,
    /// Required to explore a site that is not on this machine. The explorer clicks on the site it explores.
    #[arg(long)]
    i_understand_this_interacts: bool,
    /// Print graph.json instead of a summary. Standard output then holds that one document; progress and the run
    /// folder go to standard error.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    drivers: DriverArgs,
}

pub fn run(args: &ExploreArgs) -> ExitCode {
    let request = match build_request(args) {
        Ok(request) => request,
        Err(why) => {
            eprintln!("traceqa: {why}");
            return ExitCode::from(2);
        }
    };
    let run = match ExploreRun::prepare(request) {
        Ok(run) => run,
        Err(traceqa_engine::EngineError::Io(error)) => {
            eprintln!("traceqa: --out {}: {error}", args.out);
            return ExitCode::from(2);
        }
        Err(error) => {
            eprintln!("traceqa: {error}");
            return ExitCode::from(2);
        }
    };
    let folder = run.folder().to_path_buf();
    static HANDLER: Once = Once::new();
    HANDLER.call_once(|| {
        interrupt::install(interrupt::Hooks {
            first_message: "
Cancelling: closing the browser. Press Ctrl-C again to quit at once.",
            waits_for_run_folder: true,
            exits_when_idle: false,
            on_exit: || {},
            first_signal_bound: Some(interrupt::FIRST_SIGNAL_BOUND),
        });
    });
    let mut show = |event: &Event| {
        if event.type_ == "progress" {
            interrupt::say(&event.message);
        }
    };
    let finished = run.execute(&interrupt::CANCEL, &interrupt::ABORT, &mut show);

    if let Some(explored) = &finished.explored {
        if args.json {
            print!("{}", String::from_utf8_lossy(&explored.graph.to_bytes()));
            eprintln!("Run folder: {}", folder.display());
        } else {
            print_summary(&explored.graph);
            println!("\nRun folder: {}", folder.display());
        }
    } else {
        eprintln!("Run folder: {}", folder.display());
    }
    match finished.manifest.status {
        RunStatus::Completed => ExitCode::SUCCESS,
        RunStatus::Cancelled => ExitCode::from(interrupt::INTERRUPTED),
        _ => ExitCode::FAILURE,
    }
}

/// Turns the arguments into the engine's request. The error is the message for a wrong command (exit code 2); no
/// folder and no browser exist yet.
fn build_request(args: &ExploreArgs) -> Result<Request, String> {
    check_start(&args.url, &args.allow_origin, args.i_understand_this_interacts)?;
    let similarity = if (args.state_similarity - DEFAULT_SIMILARITY_SHARE).abs() < f64::EPSILON {
        DEFAULT_SIMILARITY
    } else {
        Similarity::new(args.state_similarity).map_err(|error| format!("--state-similarity: {error}"))?
    };
    let rules = Rules::with_origins(&args.url, &args.allow_origin, args.allow_writes, &args.deny_name)?;
    let storage_state = args.storage_state.as_deref().map(read_storage_state).transpose()?;

    // With the default drivers the browser names are known, so a typo fails here, before a folder exists.
    let custom_drivers = !args.drivers.drivers.is_empty()
        || std::env::var(traceqa_engine::run::DRIVERS_ENV).is_ok_and(|value| !value.trim().is_empty());
    if !custom_drivers {
        args.browser.check_known().map_err(|error| error.to_string())?;
    }
    let mut target = args.browser.clone();
    target.headless = Some(!args.headed);

    if !traceqa_engine::policy::is_local_origin(&args.url) {
        eprintln!(
            "traceqa: exploring {} which is not on this machine{}.",
            args.url,
            if args.allow_writes {
                ", with writes allowed"
            } else {
                " (read-only: the browser refuses every request that is not GET or HEAD)"
            }
        );
    }
    Ok(Request {
        options: Options {
            start_url: args.url.clone(),
            strategy: match args.strategy {
                StrategyArg::Bfs => Strategy::Bfs,
                StrategyArg::Dfs => Strategy::Dfs,
            },
            seed: args.seed,
            similarity,
            budgets: Budgets {
                max_states: args.max_states,
                max_actions: args.max_actions,
                max_depth: args.max_depth,
                action_timeout: Duration::from_secs(args.action_timeout),
                timeout: Duration::from_secs(args.timeout),
                max_requests_per_origin: args.max_requests_per_origin,
            },
            rules,
            chrome_pages: args.chrome_pages,
        },
        target,
        viewport: args.viewport.clone(),
        storage_state,
        out_dir: PathBuf::from(&args.out),
        drivers: driver_commands(&args.drivers.drivers),
    })
}

/// The share `--state-similarity` has when it is not given, so the default is the engine's own constant exactly.
const DEFAULT_SIMILARITY_SHARE: f64 = 0.9;

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

fn print_summary(graph: &traceqa_engine::explore::Graph) {
    let touched = graph.edges.iter().filter(|e| e.status.as_str() == "ok").count();
    println!(
        "\nExplored {}: {} screens, {} actions ({} worked), stopped: {}",
        graph.settings.start_url,
        graph.states.len(),
        graph.actions,
        touched,
        graph.stopped.as_str()
    );
    if !graph.skipped.is_empty() {
        let mut reasons: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for item in &graph.skipped {
            *reasons.entry(item.reason.as_str()).or_default() += 1;
        }
        let list: Vec<String> = reasons.iter().map(|(reason, n)| format!("{n} {reason}")).collect();
        println!("Left alone: {} ({})", graph.skipped.len(), list.join(", "));
    }
    if !graph.unreachable.is_empty() {
        println!(
            "Could not be opened again: {} (what was left on them was not tried)",
            graph.unreachable.join(", ")
        );
    }
    if graph.stopped == Stop::MaxDepth {
        println!(
            "Some screens are deeper than --max-depth {} and were not explored.",
            graph.settings.max_depth
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Wrapper {
        #[command(flatten)]
        args: ExploreArgs,
    }

    fn build(extra: &[&str]) -> Result<Request, String> {
        let mut argv = vec!["explore"];
        argv.extend_from_slice(extra);
        build_request(&Wrapper::try_parse_from(argv).map_err(|e| e.to_string())?.args)
    }

    #[test]
    fn a_site_that_is_not_local_is_refused_and_the_message_names_the_flag() {
        let why = build(&["https://example.com/"]).err().unwrap();
        assert!(why.contains("--i-understand-this-interacts"), "{why}");
        assert!(build(&["https://example.com/", "--i-understand-this-interacts"]).is_ok());
    }

    #[test]
    fn a_local_site_needs_no_flag() {
        assert!(build(&["http://127.0.0.1:4173/"]).is_ok());
        assert!(build(&["http://shop.test/"]).is_ok());
    }

    #[test]
    fn writes_need_the_origin_named() {
        let why = build(&["http://127.0.0.1:4173/", "--allow-writes"]).err().unwrap();
        assert!(why.contains("--allow-origin"), "{why}");
        assert!(build(&[
            "http://127.0.0.1:4173/",
            "--allow-writes",
            "--allow-origin",
            "http://127.0.0.1:4173"
        ])
        .is_ok());
    }

    #[test]
    fn the_defaults_are_the_ones_the_backlog_gives() {
        let request = build(&["http://127.0.0.1:4173/"]).unwrap();
        let budgets = request.options.budgets;
        assert_eq!(budgets, Budgets::default());
        assert_eq!(request.options.similarity, DEFAULT_SIMILARITY);
        assert_eq!(request.options.seed, 0);
        assert!(matches!(request.options.strategy, Strategy::Bfs));
    }

    #[test]
    fn a_similarity_outside_zero_to_one_is_refused() {
        assert!(build(&["http://127.0.0.1:4173/", "--state-similarity", "1.5"]).is_err());
        assert!(build(&["http://127.0.0.1:4173/", "--state-similarity", "1.0"]).is_ok());
    }

    #[test]
    fn something_that_is_not_an_address_is_refused() {
        assert!(build(&["not an address"]).is_err());
        assert!(build(&["ftp://127.0.0.1/"]).is_err());
    }

    #[test]
    fn a_storage_file_that_is_not_one_is_refused_without_quoting_it() {
        let dir = std::env::temp_dir().join(format!("tq-explore-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("state.json");
        std::fs::write(&file, "SECRET-TOKEN not json").unwrap();
        let why = build(&["http://127.0.0.1:4173/", "--storage-state", file.to_str().unwrap()])
            .err()
            .unwrap();
        assert!(!why.contains("SECRET-TOKEN"), "{why}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
