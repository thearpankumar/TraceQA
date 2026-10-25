//! The engine's side of the client protocol: `traceqa serve`.
//!
//! A client writes requests to our input, one per line. We write responses and
//! notifications to our output. A run executes on its own thread, so the client
//! can cancel it or start another while it is going.

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use traceqa_protocol::contracts::client_protocol::{method, notification, VERSION as PROTOCOL_VERSION};
use traceqa_protocol::jsonrpc::{code, serve_lines, Message, Output, RpcError};
use traceqa_protocol::supervise::guard;
use traceqa_protocol::sync::LockExt;
use traceqa_protocol::types::{
    ComponentsFile, DiffCompareParams, EngineHelloParams, EngineHelloResult, Event, FindingsAnalyseParams,
    FindingsFile, FindingsResult, RpcErrorData, RunCancelParams, RunDoneParams, RunEventParams, RunManifest,
    RunStartParams, RunStartResult, RunStatus,
};

use crate::driver::{DriverCommand, Drivers, ErrorKind};
use crate::run::{failed_after_panic, Execution, Interrupt, Run, FEATURES};

fn failure(rpc_code: i64, kind: &ErrorKind, message: impl Into<String>) -> RpcError {
    failure_with(rpc_code, kind, false, message)
}

fn failure_with(rpc_code: i64, kind: &ErrorKind, retryable: bool, message: impl Into<String>) -> RpcError {
    let data = RpcErrorData {
        kind: kind.as_str().to_string(),
        retryable,
    };
    RpcError {
        code: rpc_code,
        message: message.into(),
        data: serde_json::to_value(data).ok(),
    }
}

fn parse<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    serde_json::from_value(params).map_err(|e| failure(code::INVALID_PARAMS, &ErrorKind::InvalidParams, e.to_string()))
}

fn answer(result: impl Serialize) -> Result<Value, RpcError> {
    Ok(serde_json::to_value(result).expect("spec types always serialise"))
}

/// One of the runs being counted against `--max-runs`. Dropped when the run ends, also when its
/// thread panics.
struct RunSlot(Arc<AtomicUsize>);

impl Drop for RunSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A run that was started and may still be going.
struct ActiveRun {
    cancel: Arc<AtomicBool>,
    thread: JoinHandle<()>,
    /// Held until the client has been told the run was accepted. The run waits for it,
    /// so its first event can never overtake that answer.
    go: Option<Sender<()>>,
}

/// How many runs `traceqa serve` lets go at once unless told otherwise.
pub const DEFAULT_MAX_RUNS: usize = 4;

/// Limits of one serving engine.
#[derive(Debug, Clone)]
pub struct ServeOptions {
    /// How many runs may be going at once. A `run.start` beyond this is refused with a
    /// retryable `busy` error. At least 1 is used.
    pub max_runs: usize,
    /// The process's signal state. When its `cancel` flag is set, every run is cancelled through
    /// the same path as `run.cancel`, and `run.start` is refused. Its `abort` ends the runs at
    /// once. The process, not this function, exits afterwards: the input may be blocked on a
    /// read that nothing will end. `None` for a server that reacts to its input only.
    pub interrupt: Option<Interrupt>,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            max_runs: DEFAULT_MAX_RUNS,
            interrupt: None,
        }
    }
}

/// How often the interrupt monitor looks at the signal flag.
const INTERRUPT_POLL: Duration = Duration::from_millis(20);

/// The cancel flags of the runs that were started, for the interrupt monitor.
type Flags = Arc<Mutex<Vec<Arc<AtomicBool>>>>;

/// What starting a run needs besides the request.
struct Context<'a> {
    make_drivers: &'a MakeDrivers,
    output: &'a Output,
    active: &'a Arc<AtomicUsize>,
    max_runs: usize,
    interrupt: Option<Interrupt>,
    flags: &'a Flags,
}

/// Cancels every registered run as soon as `interrupt.cancel` is set, until `done` is. It keeps
/// storing on every tick, so a run registered in the moment after the signal is caught too.
fn monitor(interrupt: Interrupt, flags: Flags, done: Arc<AtomicBool>) -> JoinHandle<()> {
    thread::spawn(move || {
        while !done.load(Ordering::SeqCst) {
            if interrupt.cancel.load(Ordering::SeqCst) {
                for flag in flags.lock_ok().iter() {
                    flag.store(true, Ordering::Relaxed);
                }
            }
            thread::sleep(INTERRUPT_POLL);
        }
    })
}

/// Makes the drivers one run's workers use.
type MakeDrivers = Arc<dyn Fn() -> Drivers + Send + Sync>;

/// Serves clients until `input` ends, then waits for running runs to finish.
pub fn serve(input: impl BufRead, output: Box<dyn Write + Send>, drivers: Vec<DriverCommand>) -> io::Result<()> {
    serve_with(input, output, drivers, &ServeOptions::default())
}

/// As [`serve`], with limits.
pub fn serve_with(
    input: impl BufRead,
    output: Box<dyn Write + Send>,
    drivers: Vec<DriverCommand>,
    options: &ServeOptions,
) -> io::Result<()> {
    let make: MakeDrivers = Arc::new(move || Drivers::new(drivers.clone()));
    serve_inner(input, output, &make, options)
}

fn serve_inner(
    input: impl BufRead,
    output: Box<dyn Write + Send>,
    make_drivers: &MakeDrivers,
    options: &ServeOptions,
) -> io::Result<()> {
    let output = Output::new(output);
    let mut runs: HashMap<String, ActiveRun> = HashMap::new();
    let active = Arc::new(AtomicUsize::new(0));
    let max_runs = options.max_runs.max(1);
    let flags: Flags = Arc::default();
    let monitor_done = Arc::new(AtomicBool::new(false));
    let monitoring = options
        .interrupt
        .map(|interrupt| monitor(interrupt, Arc::clone(&flags), Arc::clone(&monitor_done)));
    let context = Context {
        make_drivers,
        output: &output,
        active: &active,
        max_runs,
        interrupt: options.interrupt,
        flags: &flags,
    };
    // Set by engine.shutdown: the client is closing on purpose and wants no new runs.
    let mut shutting_down = false;

    let reading = serve_lines(
        input,
        &output,
        |rpc_code, message| failure(rpc_code, &ErrorKind::InvalidRequest, message),
        |message| {
            // We never receive anything but requests that need an answer.
            let Message::Request { id, method, params } = message else {
                return;
            };
            // Forget runs that have finished, so an engine that serves for a long time does not grow.
            runs.retain(|_, run| !run.thread.is_finished());

            let result = match method.as_str() {
                method::ENGINE_HELLO => parse(params).and_then(|request: EngineHelloParams| hello(&request)),
                method::RUN_START
                    if shutting_down || context.interrupt.is_some_and(|i| i.cancel.load(Ordering::SeqCst)) =>
                {
                    Err(failure(
                        code::APPLICATION_ERROR,
                        &ErrorKind::Busy,
                        "the engine is shutting down and starts no new runs",
                    ))
                }
                method::RUN_START => parse(params).and_then(|request| start_run(request, &context, &mut runs)),
                // Read local files and answer when done, inline: the size caps keep this short.
                method::DIFF_COMPARE => parse(params).and_then(|request: DiffCompareParams| diff_compare(&request)),
                method::FINDINGS_ANALYSE => {
                    parse(params).and_then(|request: FindingsAnalyseParams| findings_analyse(&request))
                }
                method::ENGINE_SHUTDOWN => {
                    shutting_down = true;
                    answer(serde_json::json!({}))
                }
                method::RUN_CANCEL => {
                    parse(params).and_then(|request: RunCancelParams| match runs.get(&request.run_id) {
                        Some(run) => {
                            cancel(run);
                            answer(serde_json::json!({}))
                        }
                        None => Err(failure(
                            code::INVALID_PARAMS,
                            &ErrorKind::InvalidParams,
                            format!("no running run {}", request.run_id),
                        )),
                    })
                }
                other => Err(failure(
                    code::METHOD_NOT_FOUND,
                    &ErrorKind::Unsupported,
                    format!("unknown method {other}"),
                )),
            };
            output.send(&Message::Response { id, result });
            for run in runs.values_mut() {
                run.go.take();
            }
        },
    );

    // The input ended. After engine.shutdown the client meant to close, so its runs finish. Without it
    // the client may have died, so nobody wants the results: cancel every run, through the same path as
    // run.cancel. Either way the threads are joined, so every run folder is finalised before we return.
    if !shutting_down {
        cancel_every(&runs);
    }
    for run in runs.into_values() {
        drop(run.go);
        let _ = run.thread.join();
    }
    monitor_done.store(true, Ordering::SeqCst);
    if let Some(monitoring) = monitoring {
        let _ = monitoring.join();
    }
    reading
}

/// Asks a run to stop. It ends as cancelled, with its folder finalised.
fn cancel(run: &ActiveRun) {
    run.cancel.store(true, Ordering::Relaxed);
}

fn cancel_every(runs: &HashMap<String, ActiveRun>) {
    runs.values().for_each(cancel);
}

fn hello(hello: &EngineHelloParams) -> Result<Value, RpcError> {
    if hello.protocol_version != PROTOCOL_VERSION {
        return Err(failure(
            code::APPLICATION_ERROR,
            &ErrorKind::VersionMismatch,
            format!(
                "engine speaks client protocol {PROTOCOL_VERSION}, client asked for {}",
                hello.protocol_version
            ),
        ));
    }
    answer(EngineHelloResult {
        engine_version: crate::VERSION.to_string(),
        protocol_version: PROTOCOL_VERSION,
        features: HELLO_FEATURES.iter().map(std::string::ToString::to_string).collect(),
    })
}

/// What `engine.hello` lists: the features `run.start` takes, then the methods that read run
/// folders. A client that needs `diff.compare` or `findings.analyse` looks for them here.
const HELLO_FEATURES: [&str; 3] = [FEATURES[0], "diff", "findings"];

/// The largest `components.json` that `diff.compare` and `findings.analyse` read. They run inline,
/// so a huge file would stall `run.cancel` while it is read.
pub const MAX_COMPONENTS_BYTES: u64 = 64 * 1024 * 1024;

fn invalid(message: impl Into<String>) -> RpcError {
    failure(code::INVALID_PARAMS, &ErrorKind::InvalidParams, message)
}

/// Today in UTC as `YYYY-MM-DD`, for suppression expiry.
#[must_use]
pub fn today_utc() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    civil_date(i64::try_from(seconds / 86_400).unwrap_or(0))
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

/// Reads a text file of at most `cap` bytes.
fn read_capped(path: &Path, what: &str, cap: u64) -> Result<String, RpcError> {
    let size = std::fs::metadata(path)
        .map_err(|e| invalid(format!("could not read {what}: {e}")))?
        .len();
    if size > cap {
        return Err(invalid(format!(
            "{what} is {} MiB, over the limit of {} MiB",
            size / (1024 * 1024),
            cap / (1024 * 1024)
        )));
    }
    std::fs::read_to_string(path).map_err(|e| invalid(format!("could not read {what}: {e}")))
}

/// A run folder's manifest. `name` is the parameter, for the message.
fn open_run(folder: &str, name: &str) -> Result<(PathBuf, RunManifest), RpcError> {
    let dir = PathBuf::from(folder);
    let text = read_capped(
        &dir.join("run.json"),
        &format!("run.json of {name}"),
        MAX_COMPONENTS_BYTES,
    )
    .map_err(|e| {
        invalid(format!(
            "{name} is not a run folder ({}). Pass the runFolder that run.start answered.",
            e.message
        ))
    })?;
    let manifest: RunManifest =
        serde_json::from_str(&text).map_err(|e| invalid(format!("run.json of {name} is not a run manifest: {e}")))?;
    Ok((dir, manifest))
}

/// A target folder name from run.json is data: it must be one plain name inside the run folder.
fn plain_dir(dir: &str) -> bool {
    !dir.is_empty() && dir != "." && dir != ".." && !dir.contains(['/', '\\', ':'])
}

fn read_components(folder: &Path, dir: &str, name: &str) -> Result<ComponentsFile, RpcError> {
    if !plain_dir(dir) {
        return Err(invalid(format!(
            "run.json of {name} names a target folder that is not a plain name"
        )));
    }
    let what = format!("{dir}/components.json of {name}");
    let path = folder.join(dir).join("components.json");
    if !path.is_file() {
        return Err(invalid(format!(
            "{dir} of {name} has no components.json. Check the target's status and error in run.json."
        )));
    }
    let text = read_capped(&path, &what, MAX_COMPONENTS_BYTES)?;
    serde_json::from_str(&text).map_err(|e| invalid(format!("{what} is not a components file: {e}")))
}

/// The target's `axe.json`, when the run was made with `--axe`. Its absence is normal.
fn read_axe(folder: &Path, dir: &str) -> Result<Option<traceqa_protocol::types::AxeResults>, RpcError> {
    let path = folder.join(dir).join("axe.json");
    if !path.is_file() {
        return Ok(None);
    }
    let what = format!("{dir}/axe.json");
    let text = read_capped(&path, &what, MAX_COMPONENTS_BYTES)?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| invalid(format!("{what} is not an axe file: {e}")))
}

fn target_names(manifest: &RunManifest) -> String {
    manifest
        .targets
        .iter()
        .map(|t| t.dir.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The target of a side: the one named, or the only one the run has.
fn pick_target<'m>(manifest: &'m RunManifest, wanted: Option<&str>, name: &str) -> Result<&'m str, RpcError> {
    match (wanted, manifest.targets.as_slice()) {
        (Some(wanted), targets) => targets
            .iter()
            .find(|t| t.dir == wanted)
            .map(|t| t.dir.as_str())
            .ok_or_else(|| {
                invalid(format!(
                    "{name} has no target {wanted}. It has: {}",
                    target_names(manifest)
                ))
            }),
        (None, [only]) => Ok(only.dir.as_str()),
        (None, []) => Err(invalid(format!("{name} has no targets"))),
        (None, _) => Err(invalid(format!(
            "{name} has several targets. Pass {} with one of: {}",
            if name == "a" { "targetA" } else { "targetB" },
            target_names(manifest)
        ))),
    }
}

fn diff_compare(request: &DiffCompareParams) -> Result<Value, RpcError> {
    let (dir_a, manifest_a) = open_run(&request.a, "a")?;
    let (dir_b, manifest_b) = open_run(&request.b, "b")?;
    let target_a = pick_target(&manifest_a, request.target_a.as_deref(), "a")?;
    let target_b = pick_target(&manifest_b, request.target_b.as_deref(), "b")?;
    let file_a = read_components(&dir_a, target_a, "a")?;
    let file_b = read_components(&dir_b, target_b, "b")?;
    let options = traceqa_diff::Options {
        label_a: request.a.clone(),
        label_b: request.b.clone(),
        target_a: Some(target_a.to_string()),
        target_b: Some(target_b.to_string()),
    };
    let result = traceqa_diff::compare(&file_a, &file_b, &options).map_err(|e| invalid(e.to_string()))?;
    answer(result)
}

fn findings_analyse(request: &FindingsAnalyseParams) -> Result<Value, RpcError> {
    let (dir, manifest) = open_run(&request.run, "run")?;
    let suppressions = match &request.suppressions {
        None => None,
        Some(path) => {
            let text = read_capped(
                Path::new(path),
                "the suppression file",
                traceqa_findings::suppress::MAX_FILE_BYTES as u64,
            )?;
            Some(traceqa_findings::suppress::parse(&text).map_err(|e| invalid(format!("the suppression file: {e}")))?)
        }
    };
    if let Some(wanted) = &request.target {
        let target = manifest.targets.iter().find(|t| &t.dir == wanted).ok_or_else(|| {
            invalid(format!(
                "run has no target {wanted}. It has: {}",
                target_names(&manifest)
            ))
        })?;
        if target.status != RunStatus::Completed {
            return Err(invalid(format!(
                "{wanted} did not complete, so there is nothing to analyse"
            )));
        }
    }
    let mut targets = Vec::new();
    for target in manifest.targets.iter().filter(|t| t.status == RunStatus::Completed) {
        targets.push(traceqa_findings::TargetInput {
            dir: target.dir.clone(),
            file: read_components(&dir, &target.dir, "run")?,
            axe: read_axe(&dir, &target.dir)?,
        });
    }
    let options = traceqa_findings::Options {
        min_severity: None,
        suppressions,
        today: Some(today_utc()),
    };
    let input = traceqa_findings::RunInput {
        run_id: manifest.run_id.clone(),
        targets,
    };
    // Every target is analysed so a suppression used in another browser is not reported unused;
    // only the chosen one is returned.
    let files: Vec<FindingsFile> = traceqa_findings::analyse_run(&input, &options)
        .into_iter()
        .filter(|f| match (&request.target, &f.target) {
            (Some(wanted), Some(dir)) => wanted == dir,
            (Some(_), None) => false,
            (None, _) => true,
        })
        .collect();
    answer(FindingsResult { files })
}

/// Accepts a run and executes it on its own thread. Progress and the result reach the
/// client as notifications.
fn start_run(
    request: RunStartParams,
    context: &Context,
    runs: &mut HashMap<String, ActiveRun>,
) -> Result<Value, RpcError> {
    let Context {
        make_drivers,
        output,
        active,
        max_runs,
        interrupt,
        flags,
    } = *context;
    // Before a run folder is made. The count drops before a run says it is done, so a client
    // that starts its next run on seeing run.done is not refused.
    if active.fetch_add(1, Ordering::SeqCst) >= max_runs {
        active.fetch_sub(1, Ordering::SeqCst);
        return Err(failure_with(
            code::APPLICATION_ERROR,
            &ErrorKind::Busy,
            true,
            format!("busy: {max_runs} runs are already going. Wait for one to finish, then try again."),
        ));
    }
    let counted = RunSlot(Arc::clone(active));
    let run =
        Run::prepare(request).map_err(|e| failure(code::INVALID_PARAMS, &ErrorKind::InvalidParams, e.to_string()))?;
    let run_id = run.id().to_string();
    let accepted = RunStartResult {
        run_id: run_id.clone(),
        run_folder: run.folder().to_string_lossy().into_owned(),
    };

    let cancel = Arc::new(AtomicBool::new(false));
    {
        // Runs that have ended are dropped from the list; only this list and the run hold a flag.
        let mut flags = flags.lock_ok();
        flags.retain(|flag| Arc::strong_count(flag) > 1);
        flags.push(Arc::clone(&cancel));
    }
    // Held by the run's thread until after run.done is written: a process that waits for
    // "no run in flight" before it exits then never cuts a client's last message short.
    let in_flight = interrupt.map(|interrupt| interrupt.abort.enter());
    let (go, accepted_was_sent) = mpsc::channel::<()>();
    let thread = thread::spawn({
        let (run_id, cancel, output, make_drivers) = (
            run_id.clone(),
            Arc::clone(&cancel),
            output.clone(),
            Arc::clone(make_drivers),
        );
        move || {
            // Returns when `go` is dropped, which happens right after the response is written.
            let _ = accepted_was_sent.recv();
            let mut on_event = |event: &Event| {
                let params = RunEventParams {
                    run_id: run_id.clone(),
                    event: event.clone(),
                };
                output.notify(notification::RUN_EVENT, &params);
            };
            let execution = Execution {
                drivers: &|| make_drivers(),
                cancel: &cancel,
                policy: None,
            };
            // `execute` supervises its own work; this is the second belt, for a panic that gets
            // out of it, so the client is always told the run is done.
            let folder = run.folder().to_path_buf();
            let manifest = guard(|| match interrupt {
                Some(interrupt) => run.execute_abortable(&execution, interrupt.abort, &mut on_event),
                None => run.execute(&execution, &mut on_event),
            })
            .unwrap_or_else(|message| failed_after_panic(&folder, &run_id, &message));
            drop(counted);
            let done = RunDoneParams {
                run_id: run_id.clone(),
                status: manifest.status,
                manifest,
            };
            output.notify(notification::RUN_DONE, &done);
            drop(in_flight);
        }
    });
    runs.insert(
        run_id,
        ActiveRun {
            cancel,
            thread,
            go: Some(go),
        },
    );
    answer(accepted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Collects what the engine writes, so a test can read it after `serve` returns.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl Write for Captured {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Feeds `requests` to the engine and returns every message it sent back.
    fn converse(requests: &[Value]) -> Vec<Value> {
        let input: String = requests.iter().map(|request| format!("{request}\n")).collect();
        let captured = Captured::default();
        // No such driver exists, so a run that is accepted fails without opening a browser.
        let drivers = vec![DriverCommand::parse("traceqa-no-such-driver").unwrap()];
        serve(input.as_bytes(), Box::new(captured.clone()), drivers).unwrap();
        let bytes = captured.0.lock().unwrap().clone();
        String::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    // Callers pass `json!` literals; taking them by value keeps every call site short.
    #[allow(clippy::needless_pass_by_value)]
    fn request(id: u64, method: &str, params: Value) -> Value {
        serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
    }

    #[test]
    fn requests_are_answered_or_refused_with_a_reason() {
        let replies = converse(&[
            request(
                1,
                "engine.hello",
                serde_json::json!({ "protocolVersion": PROTOCOL_VERSION }),
            ),
            request(2, "engine.hello", serde_json::json!({ "protocolVersion": 999 })),
            request(
                3,
                "run.start",
                serde_json::json!({ "feature": "extract", "url": "nope", "targets": [], "viewports": [] }),
            ),
            request(4, "run.cancel", serde_json::json!({ "runId": "r_missing" })),
            request(5, "run.fly", serde_json::json!({})),
            request(6, "run.start", serde_json::json!({ "url": 7 })),
        ]);
        assert_eq!(replies.len(), 6);
        assert_eq!(
            replies[0]["result"]["features"],
            serde_json::json!(["extract", "diff", "findings"])
        );
        let kinds: Vec<&str> = replies[1..]
            .iter()
            .map(|reply| reply["error"]["data"]["kind"].as_str().unwrap())
            .collect();
        assert_eq!(
            kinds,
            [
                "version_mismatch",
                "invalid_params",
                "invalid_params",
                "unsupported",
                "invalid_params"
            ]
        );
        assert_eq!(replies[4]["error"]["code"], code::METHOD_NOT_FOUND);
    }

    #[test]
    fn a_line_that_cannot_be_read_is_refused_and_serving_continues() {
        let mut input = b"this is not json\n\xff\xfe not utf-8\n".to_vec();
        input.extend(
            format!(
                "{}\n",
                request(
                    7,
                    "engine.hello",
                    serde_json::json!({ "protocolVersion": PROTOCOL_VERSION })
                )
            )
            .bytes(),
        );
        let captured = Captured::default();
        serve(input.as_slice(), Box::new(captured.clone()), vec![]).unwrap();
        let bytes = captured.0.lock().unwrap().clone();
        let replies: Vec<Value> = String::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();

        assert_eq!(replies.len(), 3);
        for refused in &replies[..2] {
            assert!(refused["id"].is_null());
            assert_eq!(refused["error"]["code"], code::PARSE_ERROR);
        }
        assert_eq!(replies[2]["id"], 7);
        assert!(replies[2]["result"]["engineVersion"].is_string());
    }

    #[test]
    fn a_run_is_accepted_at_once_and_reports_its_end_as_a_notification() {
        let out_dir = std::env::temp_dir().join("traceqa-serve-tests");
        let start = serde_json::json!({
            "feature": "extract",
            "url": "https://shop.test",
            "targets": [{ "browser": "firefox" }],
            "viewports": [{ "width": 1280, "height": 800 }],
            "outDir": out_dir,
        });
        // A client that closes on purpose says so, or its run would be cancelled when the input ends.
        let replies = converse(&[
            request(1, "run.start", start),
            request(2, "engine.shutdown", serde_json::json!({})),
        ]);

        // The response comes first and only says the run was accepted.
        let run_id = replies[0]["result"]["runId"].as_str().unwrap();
        assert_eq!(replies[0]["id"], 1);
        // Everything after it is a notification about that run, ending with run.done.
        let methods: Vec<&str> = replies[1..]
            .iter()
            .filter(|message| message["id"] != 2)
            .map(|message| message["method"].as_str().unwrap())
            .collect();
        assert!(methods.len() >= 2 && methods[..methods.len() - 1].iter().all(|method| *method == "run.event"));
        let done = replies.last().unwrap();
        assert_eq!(done["method"], "run.done");
        assert_eq!(done["params"]["runId"], run_id);
        assert_eq!(done["params"]["status"], "failed");
        let error = done["params"]["manifest"]["targets"][0]["error"].as_str().unwrap();
        assert!(error.contains("no driver can open"), "{error}");
        std::fs::remove_dir_all(replies[0]["result"]["runFolder"].as_str().unwrap()).unwrap();
    }

    #[test]
    fn run_start_with_a_bad_viewport_answers_invalid_params_at_once() {
        for (width, height) in [(99_999_u64, 99_999_u64), (0, 0)] {
            let out_dir = std::env::temp_dir().join(format!("traceqa-serve-viewport-{width}-{}", std::process::id()));
            let start = serde_json::json!({
                "feature": "extract",
                "url": "https://shop.test",
                "targets": [{ "browser": "firefox" }],
                "viewports": [{ "width": width, "height": height }],
                "outDir": out_dir,
            });
            let replies = converse(&[
                request(1, "run.start", start),
                request(2, "engine.shutdown", serde_json::json!({})),
            ]);
            assert_eq!(replies[0]["error"]["code"], code::INVALID_PARAMS, "{replies:?}");
            assert_eq!(replies[0]["error"]["data"]["kind"], "invalid_params");
            let message = replies[0]["error"]["message"].as_str().unwrap();
            assert!(message.contains(&format!("viewport {width}x{height}")), "{message}");
            assert!(!out_dir.exists(), "no run folder may be made");
            // No run was accepted, so nothing but the two answers was sent.
            assert_eq!(replies.len(), 2, "{replies:?}");
        }
    }

    fn replies_in(captured: &Captured) -> Vec<Value> {
        let bytes = captured.0.lock().unwrap().clone();
        String::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// Writes into `Captured`, but panics on every `run.event` line, like a listener that breaks.
    struct PanicsOnEvents(Captured);

    impl Write for PanicsOnEvents {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            assert!(
                !String::from_utf8_lossy(bytes).contains("\"run.event\""),
                "the event listener broke"
            );
            self.0.write(bytes)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_panicking_run_still_answers_run_done_and_stays_cancellable() {
        let out_dir = std::env::temp_dir().join(format!("traceqa-serve-panic-{}", std::process::id()));
        let start = request(
            1,
            "run.start",
            serde_json::json!({
                "feature": "extract",
                "url": "https://shop.test",
                "targets": [{ "browser": "firefox" }],
                "viewports": [{ "width": 1280, "height": 800 }],
                "outDir": out_dir,
            }),
        );
        let hello = request(
            3,
            "engine.hello",
            serde_json::json!({ "protocolVersion": PROTOCOL_VERSION }),
        );
        let (server_input, mut input) = io::pipe().unwrap();
        let captured = Captured::default();
        let server = thread::spawn({
            let captured = captured.clone();
            move || {
                let make: MakeDrivers = Arc::new(|| Drivers::starting_with(vec![]));
                serve_inner(
                    io::BufReader::new(server_input),
                    Box::new(PanicsOnEvents(captured)),
                    &make,
                    &ServeOptions::default(),
                )
                .unwrap();
            }
        });
        writeln!(input, "{start}").unwrap();
        let started = std::time::Instant::now();
        let done = loop {
            let replies = replies_in(&captured);
            if let Some(done) = replies.iter().find(|reply| reply["method"] == "run.done") {
                break done.clone();
            }
            assert!(started.elapsed().as_secs() < 20, "no run.done: {replies:?}");
            thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!(done["params"]["status"], "failed", "{done}");
        let folder = replies_in(&captured)[0]["result"]["runFolder"]
            .as_str()
            .unwrap()
            .to_string();
        let on_disk: Value =
            serde_json::from_str(&std::fs::read_to_string(std::path::Path::new(&folder).join("run.json")).unwrap())
                .unwrap();
        assert_eq!(on_disk["status"], "failed");

        // The server still answers, and a cancel for the finished run is refused, not hung on.
        writeln!(
            input,
            "{}",
            request(2, "run.cancel", serde_json::json!({ "runId": done["params"]["runId"] }))
        )
        .unwrap();
        writeln!(input, "{hello}").unwrap();
        drop(input);
        server.join().unwrap();
        let replies = replies_in(&captured);
        assert!(replies
            .iter()
            .any(|reply| reply["id"] == 3 && reply["result"]["engineVersion"].is_string()));
        assert!(replies.iter().any(|reply| reply["id"] == 2));
        let _ = std::fs::remove_dir_all(out_dir);
    }

    #[test]
    fn the_same_for_the_client_protocol() {
        // 9 MiB with no newline until the end, then a request that must still be answered.
        let mut input = vec![b'a'; 9 * 1024 * 1024];
        input.push(b'\n');
        let hello = request(
            7,
            "engine.hello",
            serde_json::json!({ "protocolVersion": PROTOCOL_VERSION }),
        );
        input.extend(format!("{hello}\n").bytes());
        let captured = Captured::default();
        serve(input.as_slice(), Box::new(captured.clone()), vec![]).unwrap();
        let replies = replies_in(&captured);
        assert_eq!(replies.len(), 2);
        assert!(replies[0]["id"].is_null());
        assert_eq!(replies[0]["error"]["code"], code::INVALID_REQUEST);
        assert!(replies[0]["error"]["message"].as_str().unwrap().contains("8 MiB"));
        assert_eq!(replies[1]["id"], 7);
        assert!(replies[1]["result"]["engineVersion"].is_string());
    }

    #[test]
    fn run_start_beyond_max_runs_returns_a_busy_error_and_frees_up_after_a_run_ends() {
        use crate::driver::{DriverError, Starter};
        use std::sync::mpsc;

        // Drivers that wait until the sender is dropped, then fail: a run that is going until then.
        let (release, wait) = mpsc::channel::<()>();
        let wait = Arc::new(Mutex::new(wait));
        let make: MakeDrivers = Arc::new(move || {
            let wait = Arc::clone(&wait);
            let start: Starter = Box::new(move || {
                let _ = wait.lock().unwrap().recv();
                Err(DriverError::new(ErrorKind::Unsupported, "scripted: no browser"))
            });
            Drivers::starting_with(vec![start])
        });
        let out_dir = std::env::temp_dir().join(format!("traceqa-serve-busy-{}", std::process::id()));
        let start = |id| {
            request(
                id,
                "run.start",
                serde_json::json!({
                    "feature": "extract",
                    "url": "https://shop.test",
                    "targets": [{ "browser": "firefox" }],
                    "viewports": [{ "width": 1280, "height": 800 }],
                    "outDir": out_dir,
                }),
            )
        };

        let (server_input, mut input) = io::pipe().unwrap();
        let captured = Captured::default();
        let server = thread::spawn({
            let captured = captured.clone();
            move || {
                let options = ServeOptions {
                    max_runs: 1,
                    ..ServeOptions::default()
                };
                serve_inner(io::BufReader::new(server_input), Box::new(captured), &make, &options).unwrap();
            }
        });
        let wait_for = |what: &str, found: &dyn Fn(&[Value]) -> bool| {
            for _ in 0..1000 {
                if found(&replies_in(&captured)) {
                    return;
                }
                thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("never saw {what}: {:?}", replies_in(&captured));
        };
        let answered = |replies: &[Value], id: u64| replies.iter().any(|reply| reply["id"] == id);

        writeln!(input, "{}", start(1)).unwrap();
        writeln!(input, "{}", start(2)).unwrap();
        wait_for("both answers", &|replies| answered(replies, 1) && answered(replies, 2));
        let replies = replies_in(&captured);
        let first = replies.iter().find(|reply| reply["id"] == 1).unwrap();
        assert!(first["result"]["runId"].is_string(), "{first}");
        let second = replies.iter().find(|reply| reply["id"] == 2).unwrap();
        assert_eq!(second["error"]["code"], code::APPLICATION_ERROR);
        assert_eq!(second["error"]["data"]["kind"], "busy");
        assert_eq!(second["error"]["data"]["retryable"], true);

        // Let the first run end; the next run.start is accepted as soon as run.done is seen.
        drop(release);
        wait_for("run.done", &|replies| {
            replies.iter().any(|reply| reply["method"] == "run.done")
        });
        writeln!(input, "{}", start(3)).unwrap();
        wait_for("the third answer", &|replies| answered(replies, 3));
        let replies = replies_in(&captured);
        let third = replies.iter().find(|reply| reply["id"] == 3).unwrap();
        assert!(third["result"]["runId"].is_string(), "{third}");

        drop(input);
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(out_dir);
    }

    /// Drivers whose start blocks until `release` is dropped, then fails. The first start also says it began.
    fn slow_drivers(started: mpsc::Sender<()>, release: mpsc::Receiver<()>) -> MakeDrivers {
        use crate::driver::{DriverError, Starter};
        let started = Mutex::new(started);
        let release = Arc::new(Mutex::new(release));
        Arc::new(move || {
            let release = Arc::clone(&release);
            let started = started.lock().unwrap().clone();
            let start: Starter = Box::new(move || {
                let _ = started.send(());
                let _ = release.lock().unwrap().recv();
                Err(DriverError::new(ErrorKind::Unsupported, "scripted: no browser"))
            });
            Drivers::starting_with(vec![start])
        })
    }

    fn two_target_start(id: u64, out_dir: &std::path::Path) -> Value {
        request(
            id,
            "run.start",
            serde_json::json!({
                "feature": "extract",
                "url": "https://shop.test",
                "targets": [{ "browser": "firefox" }],
                "viewports": [{ "width": 1280, "height": 800 }, { "width": 390, "height": 844 }],
                "parallel": 1,
                "outDir": out_dir,
            }),
        )
    }

    /// Serves on a pipe in a thread. Returns the way to write to it and to wait for it to return.
    fn serve_on_a_pipe(make: MakeDrivers, captured: &Captured) -> (io::PipeWriter, JoinHandle<()>) {
        serve_on_a_pipe_with(make, captured, ServeOptions::default())
    }

    fn serve_on_a_pipe_with(
        make: MakeDrivers,
        captured: &Captured,
        options: ServeOptions,
    ) -> (io::PipeWriter, JoinHandle<()>) {
        let (server_input, input) = io::pipe().unwrap();
        let server = thread::spawn({
            let captured = captured.clone();
            move || {
                serve_inner(io::BufReader::new(server_input), Box::new(captured), &make, &options).unwrap();
            }
        });
        (input, server)
    }

    fn run_json(captured: &Captured) -> Value {
        let replies = replies_in(captured);
        let folder = replies[0]["result"]["runFolder"].as_str().unwrap();
        serde_json::from_str(&std::fs::read_to_string(std::path::Path::new(folder).join("run.json")).unwrap()).unwrap()
    }

    #[test]
    fn eof_without_shutdown_cancels_running_runs_and_finalises_folders() {
        let (started, started_rx) = mpsc::channel();
        let (release, wait) = mpsc::channel::<()>();
        let out_dir = std::env::temp_dir().join(format!("traceqa-serve-eof-{}", std::process::id()));
        let captured = Captured::default();
        let (mut input, server) = serve_on_a_pipe(slow_drivers(started, wait), &captured);
        writeln!(input, "{}", two_target_start(1, &out_dir)).unwrap();
        started_rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
        // The client dies: its end of the pipe closes with no engine.shutdown.
        drop(input);
        thread::sleep(std::time::Duration::from_millis(300));
        // The first browser start is still blocked. Once it ends, the cancelled run must not start the second target.
        drop(release);
        server.join().unwrap();

        let done = replies_in(&captured)
            .into_iter()
            .find(|reply| reply["method"] == "run.done")
            .unwrap();
        assert_eq!(done["params"]["status"], "cancelled", "{done}");
        let on_disk = run_json(&captured);
        assert_eq!(on_disk["status"], "cancelled");
        assert!(on_disk["finishedAt"].is_string(), "{on_disk}");
        let _ = std::fs::remove_dir_all(out_dir);
    }

    #[test]
    fn the_interrupt_cancels_the_run_with_the_input_still_open_and_refuses_new_runs() {
        use crate::run::Abort;
        // Statics of their own: tests never touch the process's real signal state.
        let cancel: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
        let abort: &'static Abort = Box::leak(Box::new(Abort::new()));
        let options = ServeOptions {
            interrupt: Some(Interrupt { cancel, abort }),
            ..ServeOptions::default()
        };
        let (started, started_rx) = mpsc::channel();
        let (release, wait) = mpsc::channel::<()>();
        let out_dir = std::env::temp_dir().join(format!("traceqa-serve-signal-{}", std::process::id()));
        let captured = Captured::default();
        let (mut input, server) = serve_on_a_pipe_with(slow_drivers(started, wait), &captured, options);
        writeln!(input, "{}", two_target_start(1, &out_dir)).unwrap();
        started_rx.recv_timeout(Duration::from_secs(20)).unwrap();
        assert!(!abort.wait_idle(Duration::ZERO), "a run is in flight");

        // The signal arrives; nobody closes the input.
        cancel.store(true, Ordering::SeqCst);
        thread::sleep(Duration::from_millis(200));
        drop(release);
        assert!(abort.wait_idle(Duration::from_secs(20)), "the run ended and said so");
        let done = replies_in(&captured)
            .into_iter()
            .find(|reply| reply["method"] == "run.done")
            .expect("run.done was written before the run stopped being in flight");
        assert_eq!(done["params"]["status"], "cancelled", "{done}");
        let on_disk = run_json(&captured);
        assert_eq!(on_disk["status"], "cancelled");
        assert!(on_disk["finishedAt"].is_string(), "{on_disk}");

        writeln!(input, "{}", two_target_start(2, &out_dir)).unwrap();
        let refused = loop {
            if let Some(reply) = replies_in(&captured).into_iter().find(|reply| reply["id"] == 2) {
                break reply;
            }
            thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(refused["error"]["data"]["kind"], "busy", "{refused}");
        assert!(refused["error"]["message"].as_str().unwrap().contains("shutting down"));

        drop(input);
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(out_dir);
    }

    #[test]
    fn shutdown_request_lets_runs_finish_then_exits() {
        let (started, started_rx) = mpsc::channel();
        let (release, wait) = mpsc::channel::<()>();
        let out_dir = std::env::temp_dir().join(format!("traceqa-serve-shutdown-{}", std::process::id()));
        let captured = Captured::default();
        let (mut input, server) = serve_on_a_pipe(slow_drivers(started, wait), &captured);
        writeln!(input, "{}", two_target_start(1, &out_dir)).unwrap();
        started_rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
        writeln!(input, "{}", request(2, "engine.shutdown", serde_json::json!({}))).unwrap();
        drop(input);
        thread::sleep(std::time::Duration::from_millis(300));
        drop(release);
        server.join().unwrap();

        let replies = replies_in(&captured);
        let answer = replies.iter().find(|reply| reply["id"] == 2).unwrap();
        assert_eq!(answer["result"], serde_json::json!({}));
        // Nobody cancelled it, so both targets ran (and failed, as the scripted driver cannot start).
        let done = replies.iter().find(|reply| reply["method"] == "run.done").unwrap();
        assert_eq!(done["params"]["status"], "failed", "{done}");
        assert_eq!(run_json(&captured)["status"], "failed");
        let _ = std::fs::remove_dir_all(out_dir);
    }

    #[test]
    fn run_start_after_shutdown_is_refused() {
        /// Removes the folder when the test ends, also when an assertion panics.
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        // One folder per process, so parallel test processes do not meet; any leftover from a
        // crashed earlier run is cleared first, and a folder of another process is left alone.
        let out_dir = std::env::temp_dir().join(format!("traceqa-serve-after-shutdown-{}", std::process::id()));
        let stale = std::env::temp_dir().join(format!("traceqa-serve-after-shutdown-{}", u32::MAX));
        std::fs::create_dir_all(&stale).unwrap();
        let _stale = Cleanup(stale);
        let _ = std::fs::remove_dir_all(&out_dir);
        let _cleanup = Cleanup(out_dir.clone());
        let replies = converse(&[
            request(1, "engine.shutdown", serde_json::json!({})),
            two_target_start(2, &out_dir),
            request(
                3,
                "engine.hello",
                serde_json::json!({ "protocolVersion": PROTOCOL_VERSION }),
            ),
        ]);
        assert_eq!(replies.len(), 3, "{replies:?}");
        assert_eq!(replies[0]["result"], serde_json::json!({}));
        assert_eq!(replies[1]["error"]["code"], code::APPLICATION_ERROR);
        assert_eq!(replies[1]["error"]["data"]["kind"], "busy");
        assert!(replies[1]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("shutting down"));
        assert!(!out_dir.exists());
        assert!(replies[2]["result"]["engineVersion"].is_string());
    }

    // ---- diff.compare and findings.analyse ----

    const EXAMPLES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../spec/examples");
    const TARGET: &str = "firefox-1280x800";

    /// A run folder made from the spec examples, in a folder of its own that is removed after.
    struct Fixture(PathBuf);

    impl Fixture {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("traceqa-serve-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let target = root.join(TARGET);
            std::fs::create_dir_all(&target).unwrap();
            std::fs::copy(
                Path::new(EXAMPLES).join("RunManifest.completed.json"),
                root.join("run.json"),
            )
            .unwrap();
            std::fs::copy(
                Path::new(EXAMPLES).join("ComponentsFile.shop.json"),
                target.join("components.json"),
            )
            .unwrap();
            Self(root)
        }

        fn path(&self) -> String {
            self.0.display().to_string()
        }

        fn components(&self) -> PathBuf {
            self.0.join(TARGET).join("components.json")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn diff_compare_answers_with_a_diff_file() {
        let a = Fixture::new("diff-a");
        let b = Fixture::new("diff-b");
        // B has no instance that can be seen, so the one shared component changed.
        let mut file: Value = serde_json::from_str(&std::fs::read_to_string(b.components()).unwrap()).unwrap();
        for instance in file["components"][0]["instances"].as_array_mut().unwrap() {
            instance["state"]["visible"] = serde_json::json!(false);
        }
        std::fs::write(b.components(), file.to_string()).unwrap();

        let replies = converse(&[
            request(1, "diff.compare", serde_json::json!({ "a": a.path(), "b": b.path() })),
            request(
                2,
                "diff.compare",
                serde_json::json!({ "a": a.path(), "b": a.path(), "targetA": TARGET, "targetB": TARGET }),
            ),
        ]);
        let diff = &replies[0]["result"];
        assert_eq!(diff["summary"]["changed"], 1, "{replies:?}");
        assert_eq!(diff["entries"][0]["flags"][0], "became_hidden");
        assert_eq!(diff["a"]["target"], TARGET);
        assert_eq!(diff["a"]["label"], a.path());
        assert_eq!(replies[1]["result"]["summary"]["unchanged"], 1);
        assert_eq!(replies[1]["result"]["entries"], serde_json::json!([]));
    }

    #[test]
    fn findings_analyse_answers_with_one_file_per_target_and_honours_a_suppression_file() {
        let run = Fixture::new("findings");
        let mut file: Value = serde_json::from_str(&std::fs::read_to_string(run.components()).unwrap()).unwrap();
        let mut unnamed = file["components"][0].clone();
        unnamed["id"] = serde_json::json!("c_00000000bb01");
        unnamed["name"] = serde_json::json!("");
        unnamed.as_object_mut().unwrap().remove("locator");
        for instance in unnamed["instances"].as_array_mut().unwrap() {
            instance["name"] = serde_json::json!("");
        }
        file["components"].as_array_mut().unwrap().push(unnamed);
        std::fs::write(run.components(), file.to_string()).unwrap();
        let suppressions = run.0.join("suppress.yaml");
        std::fs::write(
            &suppressions,
            "suppressions:\n  - id: unnamed-control:c_00000000bb01\n    reason: Decorative.\n",
        )
        .unwrap();

        let replies = converse(&[
            request(1, "findings.analyse", serde_json::json!({ "run": run.path() })),
            request(
                2,
                "findings.analyse",
                serde_json::json!({ "run": run.path(), "target": TARGET, "suppressions": suppressions.display().to_string() }),
            ),
        ]);
        let files = replies[0]["result"]["files"].as_array().unwrap();
        assert_eq!(files[0]["scope"], "target", "{replies:?}");
        assert_eq!(files[0]["target"], TARGET);
        assert_eq!(files[0]["counts"]["error"], 1);
        assert_eq!(files[0]["findings"][0]["id"], "unnamed-control:c_00000000bb01");
        let suppressed = &replies[1]["result"]["files"][0];
        assert_eq!(suppressed["counts"]["error"], 0);
        assert_eq!(suppressed["suppressed"][0]["suppressed"]["reason"], "Decorative.");
    }

    #[test]
    fn bad_folders_targets_and_files_answer_invalid_params_with_a_fix() {
        let run = Fixture::new("bad");
        let nowhere = std::env::temp_dir()
            .join("traceqa-serve-no-such-run")
            .display()
            .to_string();
        let replies = converse(&[
            request(1, "diff.compare", serde_json::json!({ "a": nowhere, "b": run.path() })),
            request(
                2,
                "diff.compare",
                serde_json::json!({ "a": run.path(), "b": run.path(), "targetB": "webkit-1x1" }),
            ),
            request(3, "diff.compare", serde_json::json!({ "a": run.path() })),
            request(4, "findings.analyse", serde_json::json!({ "run": nowhere })),
            request(
                5,
                "findings.analyse",
                serde_json::json!({ "run": run.path(), "suppressions": nowhere }),
            ),
            request(
                6,
                "findings.analyse",
                serde_json::json!({ "run": run.path(), "target": "webkit-1x1" }),
            ),
        ]);
        let expected = [
            "is not a run folder",
            "has no target webkit-1x1",
            "missing field",
            "is not a run folder",
            "suppression file",
            "has no target webkit-1x1",
        ];
        for (reply, expected) in replies.iter().zip(expected) {
            assert_eq!(reply["error"]["code"], code::INVALID_PARAMS, "{reply}");
            assert_eq!(reply["error"]["data"]["kind"], "invalid_params");
            let message = reply["error"]["message"].as_str().unwrap();
            assert!(message.contains(expected), "{message}");
        }
        assert!(replies[0]["error"]["message"].as_str().unwrap().contains("runFolder"));
    }

    #[test]
    fn a_components_file_over_the_cap_is_refused_unread() {
        let run = Fixture::new("huge");
        // A sparse file: the size is claimed, nothing is written.
        let file = std::fs::OpenOptions::new().write(true).open(run.components()).unwrap();
        file.set_len(MAX_COMPONENTS_BYTES + 1).unwrap();
        drop(file);
        let replies = converse(&[
            request(
                1,
                "diff.compare",
                serde_json::json!({ "a": run.path(), "b": run.path() }),
            ),
            request(2, "findings.analyse", serde_json::json!({ "run": run.path() })),
        ]);
        for reply in &replies {
            assert_eq!(reply["error"]["data"]["kind"], "invalid_params", "{reply}");
            assert!(reply["error"]["message"]
                .as_str()
                .unwrap()
                .contains("over the limit of 64 MiB"));
        }
    }

    #[test]
    fn a_target_folder_that_is_not_a_plain_name_is_refused() {
        let run = Fixture::new("plain");
        let mut manifest: Value =
            serde_json::from_str(&std::fs::read_to_string(run.0.join("run.json")).unwrap()).unwrap();
        manifest["targets"][0]["dir"] = serde_json::json!("../elsewhere");
        std::fs::write(run.0.join("run.json"), manifest.to_string()).unwrap();
        let replies = converse(&[request(1, "findings.analyse", serde_json::json!({ "run": run.path() }))]);
        assert!(replies[0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not a plain name"));
    }

    #[test]
    fn dates_are_civil() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(11_016), "2000-02-29");
        assert!(crate::serve::today_utc().starts_with("20"));
    }
}
