//! `traceqa replay`: perform again the actions of an exploration's `graph.json`, and say whether each one led where it did.
//!
//! The replay is in `traceqa_engine::explore`. This file turns arguments into its request, refuses what must be refused
//! before a folder or a browser exists (exit 2), and installs the signal hooks.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Once;
use std::time::Duration;

use clap::Args;
use traceqa_common::types::{BrowserTarget, Event, RunStatus, StorageState, Viewport};
use traceqa_engine::driver_commands;
use traceqa_engine::explore::{
    chain_of, shortest_way, Doc, Mode, Outcome, ReplayOptions, ReplayRequest, ReplayRun, Replayed, MAX_GRAPH_BYTES,
};
use traceqa_engine::policy::{check_start, is_local_origin, Rules};

use crate::args::{parse_browser, parse_viewport, DriverArgs};
use crate::interrupt;

/// The types of event whose sentence is printed while the replay goes on.
const SHOWN_EVENTS: [&str; 3] = ["progress", "replay.step", "replay.reopen"];

#[derive(Args)]
pub struct ReplayArgs {
    /// The graph.json to replay, written by `traceqa explore`.
    graph: PathBuf,
    /// What to perform: edges that follow one another from the start page (`--path e0 e4 e9`), done in one session
    /// and stopped at the first that does not lead where it did; or one screen (`--path s5`), for the shortest way
    /// to it.
    #[arg(long, value_name = "ID", num_args = 1.., conflicts_with = "all")]
    path: Vec<String>,
    /// Perform every edge of the graph, each from the screen it was made on.
    #[arg(long)]
    all: bool,
    /// The site is now at this origin (scheme, host and port): every address of the graph that begins with the origin
    /// it was explored at is changed to begin with this one. For a site served on another port than before.
    #[arg(long, value_name = "ORIGIN")]
    origin: Option<String>,
    /// Browser to replay in. One per run: run the command again for another.
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
    /// Let the replay type into fields and press buttons that are not links, tabs or open-and-close controls. Without
    /// it the browser refuses every request that is not GET or HEAD, and such an edge is `blocked`. Needs
    /// --allow-origin for this site.
    #[arg(long)]
    allow_writes: bool,
    /// An origin the replay may go to (scheme, host and port). Repeat for several. With --allow-writes, the start
    /// site must be one of them.
    #[arg(long, value_name = "ORIGIN")]
    allow_origin: Vec<String>,
    /// A word or phrase that marks a control as not to be touched, in any mode. Repeat for several.
    #[arg(long, value_name = "WORD")]
    deny_name: Vec<String>,
    /// The most one action may take, in seconds.
    #[arg(long, value_name = "SECONDS", default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..=300))]
    action_timeout: u64,
    /// The most the whole replay may take, in seconds.
    #[arg(long, value_name = "SECONDS", default_value_t = 600, value_parser = clap::value_parser!(u64).range(1..=86_400))]
    timeout: u64,
    /// Stop when the browser has sent this many requests to one origin.
    #[arg(long, value_name = "N", default_value_t = 2000, value_parser = clap::value_parser!(u64).range(1..))]
    max_requests_per_origin: u64,
    /// Required to replay on a site that is not on this machine. The replay clicks on the site.
    #[arg(long)]
    i_understand_this_interacts: bool,
    /// Print replay.json instead of a summary. Standard output then holds that one document; progress and the run
    /// folder go to standard error.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    drivers: DriverArgs,
}

/// Exit code: 0 when every step that could be replayed was reproduced, 1 when any was not or the replay did not finish,
/// 2 for a wrong command or a graph that cannot be used, 130 after Ctrl-C.
pub fn run(args: &ReplayArgs) -> ExitCode {
    let request = match build_request(args) {
        Ok(request) => request,
        Err(why) => {
            eprintln!("traceqa: {why}");
            return ExitCode::from(2);
        }
    };
    let (doc, mode) = (request.doc.clone(), request.mode.clone());
    let run = match ReplayRun::prepare(request) {
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
        if SHOWN_EVENTS.contains(&event.type_.as_str()) {
            interrupt::say(&event.message);
        }
    };
    let finished = run.execute(&interrupt::CANCEL, &interrupt::ABORT, &mut show);

    if let Some(replayed) = &finished.replayed {
        if args.json {
            print!("{}", String::from_utf8_lossy(&replayed.to_bytes(&doc, &mode)));
            eprintln!("Run folder: {}", folder.display());
        } else {
            print_summary(&doc, replayed);
            println!("\nRun folder: {}", folder.display());
        }
    } else {
        eprintln!("Run folder: {}", folder.display());
    }
    match finished.manifest.status {
        RunStatus::Completed if finished.replayed.as_ref().is_some_and(Replayed::succeeded) => ExitCode::SUCCESS,
        RunStatus::Cancelled => ExitCode::from(interrupt::INTERRUPTED),
        _ => ExitCode::FAILURE,
    }
}

/// Turns the arguments into the engine's request. The error is the message for a wrong command (exit code 2); no
/// folder and no browser exist yet.
fn build_request(args: &ReplayArgs) -> Result<ReplayRequest, String> {
    if !args.all && args.path.is_empty() {
        return Err(
            "say what to replay: --path e0 e1 ... (edges that follow one another), --path sN (a screen) or --all"
                .to_string(),
        );
    }
    let mut doc = read_graph(&args.graph)?;
    if let Some(origin) = &args.origin {
        doc.rebase(origin).map_err(|error| format!("--origin: {error}"))?;
    }
    if doc.start_is_hidden() {
        return Err(
            "the graph's start address has parameters that were hidden when it was written, so it cannot be opened \
             again. Explore from an address without secrets"
                .to_string(),
        );
    }
    check_start(&doc.start_url, &args.allow_origin, args.i_understand_this_interacts)?;
    let rules = Rules::with_origins(&doc.start_url, &args.allow_origin, args.allow_writes, &args.deny_name)?;
    doc.screens()
        .map_err(|error| format!("{}: {error}", args.graph.display()))?;
    let mode = mode_of(&doc, args)?;
    let storage_state = args.storage_state.as_deref().map(read_storage_state).transpose()?;

    // With the default drivers the browser names are known, so a typo fails here, before a folder exists.
    let custom_drivers = !args.drivers.drivers.is_empty()
        || std::env::var(traceqa_engine::run::DRIVERS_ENV).is_ok_and(|value| !value.trim().is_empty());
    if !custom_drivers {
        args.browser.check_known().map_err(|error| error.to_string())?;
    }
    let mut target = args.browser.clone();
    target.headless = Some(!args.headed);

    if !is_local_origin(&doc.start_url) {
        eprintln!(
            "traceqa: replaying on {} which is not on this machine{}.",
            doc.start_url,
            if args.allow_writes {
                ", with writes allowed"
            } else {
                " (read-only: the browser refuses every request that is not GET or HEAD)"
            }
        );
    }
    Ok(ReplayRequest {
        doc,
        mode,
        options: ReplayOptions {
            rules,
            action_timeout: Duration::from_secs(args.action_timeout),
            timeout: Duration::from_secs(args.timeout),
            max_requests_per_origin: args.max_requests_per_origin,
        },
        target,
        viewport: args.viewport.clone(),
        storage_state,
        out_dir: PathBuf::from(&args.out),
        drivers: driver_commands(&args.drivers.drivers),
    })
}

/// Reads the graph. A file that is too big, not there, or not a graph is the person's to fix: the message names the
/// file and what is wrong with it, never what is in it.
fn read_graph(path: &Path) -> Result<Doc, String> {
    let name = path.display();
    let size = std::fs::metadata(path)
        .map_err(|error| format!("could not read {name}: {error}"))?
        .len();
    if size > MAX_GRAPH_BYTES {
        return Err(format!(
            "{name} is {size} bytes, more than the {MAX_GRAPH_BYTES} a graph may be"
        ));
    }
    let text = std::fs::read_to_string(path).map_err(|error| format!("could not read {name}: {error}"))?;
    Doc::parse(&text).map_err(|error| format!("{name} cannot be replayed: {error}"))
}

/// What `--path` and `--all` ask for. A list of edges, or one screen.
fn mode_of(doc: &Doc, args: &ReplayArgs) -> Result<Mode, String> {
    if args.all {
        return Ok(Mode::All);
    }
    let screens = args.path.iter().filter(|name| name.starts_with('s')).count();
    if screens > 1 || (screens == 1 && args.path.len() > 1) {
        return Err("--path takes edges (e0 e1 ...) or one screen (s5), not both and not several screens".to_string());
    }
    if let [name] = args.path.as_slice() {
        if name.starts_with('s') {
            let Some(screen) = doc.state_index(name) else {
                return Err(format!("{name} is not a screen of the graph"));
            };
            if screen == 0 {
                return Err("s0 is the start page: there is nothing to replay to get there".to_string());
            }
            let way = shortest_way(doc, screen).ok_or_else(|| {
                format!("the graph has no way to {name} that is safe to repeat (edges that typed text are not used)")
            })?;
            return Ok(Mode::Chain(way));
        }
    }
    chain_of(doc, &args.path)
        .map(Mode::Chain)
        .map_err(|error| format!("--path: {error}"))
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

fn print_summary(doc: &Doc, replayed: &Replayed) {
    let totals = replayed.totals();
    println!(
        "\nReplayed {}: {} steps, {} actions, ended: {}",
        doc.start_url,
        replayed.steps.len(),
        replayed.actions,
        replayed.ended.as_str()
    );
    let share = totals.per_mille().map_or_else(
        || "nothing to replay".to_string(),
        |n| format!("{}.{}%", n / 10, n % 10),
    );
    println!(
        "Reproduced: {} of {} ({share}); diverged {}, missing {}, blocked {}, not run {}, failed in the walk {}",
        totals.reproduced,
        totals.replayable,
        totals.diverged,
        totals.missing,
        totals.blocked,
        totals.not_run,
        totals.recorded_failed
    );
    let not_reproduced: Vec<String> = replayed
        .steps
        .iter()
        .filter(|step| !matches!(step.outcome, Outcome::Reproduced | Outcome::RecordedFailed))
        .take(10)
        .map(|step| format!("{} {}", doc.edges[step.edge].id, step.outcome.as_str()))
        .collect();
    if !not_reproduced.is_empty() {
        let more = totals.replayable - totals.reproduced - not_reproduced.len();
        println!(
            "Not reproduced: {}{}",
            not_reproduced.join(", "),
            if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            }
        );
    }
    if replayed.steps.iter().any(|step| {
        step.outcome == Outcome::Blocked && step.note.as_deref().is_some_and(|note| note.contains("read-only"))
    }) {
        println!("Some steps were left alone because the replay is read-only: --allow-writes with --allow-origin lets it type and press.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use serde_json::json;

    #[derive(Parser)]
    struct Wrapper {
        #[command(flatten)]
        args: ReplayArgs,
    }

    fn graph() -> serde_json::Value {
        json!({
            "schemaVersion": 2,
            "startUrl": "http://127.0.0.1:4173/shop/",
            "stateSimilarityPerMille": 900,
            "chrome": [],
            "states": [
                { "id": "s0", "url": "http://127.0.0.1:4173/shop/", "components": ["c_a", "c_b"], "frames": [], "depth": 0 },
                { "id": "s1", "url": "http://127.0.0.1:4173/shop/cart.html", "components": ["c_c"], "frames": [], "depth": 1 },
                { "id": "s2", "url": "http://127.0.0.1:4173/shop/about.html", "components": ["c_d"], "frames": [], "depth": 1 },
            ],
            "edges": [
                { "id": "e0", "from": "s0", "component": "c_a", "label": "link \"Cart\"", "action": "click", "status": "ok", "to": "s1" },
                { "id": "e1", "from": "s0", "component": "c_b", "label": "link \"About\"", "action": "click", "status": "ok", "to": "s2" },
                { "id": "e2", "from": "s0", "component": "c_b", "label": "textbox \"City\"", "action": "type", "value": "Pune", "status": "ok", "to": "s0" },
                { "id": "e3", "from": "s1", "component": "c_c", "label": "link \"Gone\"", "action": "click", "status": "failed" },
            ],
        })
    }

    fn with_graph(extra: &[&str]) -> Result<ReplayRequest, String> {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("tq-replay-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("graph.json");
        std::fs::write(&file, graph().to_string()).unwrap();
        let mut argv = vec!["replay", file.to_str().unwrap()];
        argv.extend_from_slice(extra);
        let result = build_request(&Wrapper::try_parse_from(argv).map_err(|e| e.to_string())?.args);
        std::fs::remove_dir_all(&dir).ok();
        result
    }

    #[test]
    fn something_to_replay_must_be_named() {
        let why = with_graph(&[]).err().unwrap();
        assert!(why.contains("--path") && why.contains("--all"), "{why}");
    }

    #[test]
    fn all_and_a_path_cannot_be_given_together() {
        assert!(with_graph(&["--all", "--path", "e0"]).is_err());
    }

    #[test]
    fn edges_that_follow_one_another_are_a_chain() {
        let request = with_graph(&["--path", "e0"]).unwrap();
        assert_eq!(request.mode, Mode::Chain(vec![0]));
        assert!(matches!(with_graph(&["--all"]).unwrap().mode, Mode::All));
    }

    #[test]
    fn a_screen_is_the_shortest_way_to_it() {
        assert_eq!(with_graph(&["--path", "s2"]).unwrap().mode, Mode::Chain(vec![1]));
        assert!(with_graph(&["--path", "s0"]).err().unwrap().contains("start page"));
        assert!(with_graph(&["--path", "s9"]).err().unwrap().contains("s9"));
    }

    #[test]
    fn edges_and_a_screen_cannot_be_mixed() {
        assert!(with_graph(&["--path", "e0", "s1"]).is_err());
        assert!(with_graph(&["--path", "s1", "s2"]).is_err());
    }

    #[test]
    fn a_chain_that_does_not_follow_on_or_names_an_edge_that_failed_is_refused_with_the_reason() {
        let apart = with_graph(&["--path", "e0", "e1"]).err().unwrap();
        assert!(apart.contains("e1") && apart.contains("follow one another"), "{apart}");
        let failed = with_graph(&["--path", "e3"]).err().unwrap();
        assert!(failed.contains("failed"), "{failed}");
        assert!(with_graph(&["--path", "e99"]).err().unwrap().contains("e99"));
    }

    #[test]
    fn a_site_that_is_not_local_is_refused_and_the_message_names_the_flag() {
        let why = with_graph(&["--all", "--origin", "https://example.com"]).err().unwrap();
        assert!(why.contains("--i-understand-this-interacts"), "{why}");
        assert!(with_graph(&[
            "--all",
            "--origin",
            "https://example.com",
            "--i-understand-this-interacts"
        ])
        .is_ok());
    }

    #[test]
    fn writes_need_the_origin_named() {
        let why = with_graph(&["--all", "--allow-writes"]).err().unwrap();
        assert!(why.contains("--allow-origin"), "{why}");
        assert!(with_graph(&["--all", "--allow-writes", "--allow-origin", "http://127.0.0.1:4173"]).is_ok());
    }

    #[test]
    fn the_origin_moves_the_addresses_of_the_graph() {
        let request = with_graph(&["--all", "--origin", "http://127.0.0.1:5555"]).unwrap();
        assert_eq!(request.doc.start_url, "http://127.0.0.1:5555/shop/");
        assert_eq!(request.doc.states[1].url, "http://127.0.0.1:5555/shop/cart.html");
        assert!(with_graph(&["--all", "--origin", "ftp://x"])
            .err()
            .unwrap()
            .contains("--origin"));
    }

    #[test]
    fn a_file_that_is_not_there_or_not_a_graph_is_refused_and_not_quoted() {
        let why = build_request(
            &Wrapper::try_parse_from(["replay", "/no/such/graph.json", "--all"])
                .unwrap()
                .args,
        )
        .err()
        .unwrap();
        assert!(why.contains("/no/such/graph.json"), "{why}");
        let dir = std::env::temp_dir().join(format!("tq-replay-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("graph.json");
        std::fs::write(&file, "SECRET-TOKEN not json").unwrap();
        let why = build_request(
            &Wrapper::try_parse_from(["replay", file.to_str().unwrap(), "--all"])
                .unwrap()
                .args,
        )
        .err()
        .unwrap();
        assert!(
            why.contains("cannot be replayed") && !why.contains("SECRET-TOKEN"),
            "{why}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_start_address_with_hidden_parameters_cannot_be_opened_and_is_said_so() {
        let dir = std::env::temp_dir().join(format!("tq-replay-hidden-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("graph.json");
        let mut hidden = graph();
        hidden["startUrl"] = json!("http://127.0.0.1:4173/shop/?token=REDACTED");
        std::fs::write(&file, hidden.to_string()).unwrap();
        let why = build_request(
            &Wrapper::try_parse_from(["replay", file.to_str().unwrap(), "--all"])
                .unwrap()
                .args,
        )
        .err()
        .unwrap();
        assert!(why.contains("hidden"), "{why}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
