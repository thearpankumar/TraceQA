//! The engine's side of the client protocol: `traceqa serve`.
//!
//! A client writes requests to our input, one per line. We write responses and
//! notifications to our output. A run executes on its own thread, so the client
//! can cancel it or start another while it is going.

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use serde::Serialize;
use serde_json::Value;
use traceqa_protocol::contracts::client_protocol::{method, notification, VERSION as PROTOCOL_VERSION};
use traceqa_protocol::jsonrpc::{code, serve_lines, Message, Output, RpcError};
use traceqa_protocol::supervise::guard;
use traceqa_protocol::types::{
    EngineHelloParams, EngineHelloResult, Event, RpcErrorData, RunCancelParams, RunDoneParams, RunEventParams,
    RunStartParams, RunStartResult,
};

use crate::driver::{DriverCommand, Drivers, ErrorKind};
use crate::run::{failed_after_panic, Execution, Run, FEATURES};

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
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            max_runs: DEFAULT_MAX_RUNS,
        }
    }
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
                method::RUN_START if shutting_down => Err(failure(
                    code::APPLICATION_ERROR,
                    &ErrorKind::Busy,
                    "the engine is shutting down and starts no new runs",
                )),
                method::RUN_START => parse(params)
                    .and_then(|request| start_run(request, make_drivers, &output, &mut runs, &active, max_runs)),
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
        features: FEATURES.iter().map(std::string::ToString::to_string).collect(),
    })
}

/// Accepts a run and executes it on its own thread. Progress and the result reach the
/// client as notifications.
fn start_run(
    request: RunStartParams,
    make_drivers: &MakeDrivers,
    output: &Output,
    runs: &mut HashMap<String, ActiveRun>,
    active: &Arc<AtomicUsize>,
    max_runs: usize,
) -> Result<Value, RpcError> {
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
            let manifest = guard(|| run.execute(&execution, &mut on_event))
                .unwrap_or_else(|message| failed_after_panic(&folder, &run_id, &message));
            drop(counted);
            let done = RunDoneParams {
                run_id: run_id.clone(),
                status: manifest.status,
                manifest,
            };
            output.notify(notification::RUN_DONE, &done);
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
        assert_eq!(replies[0]["result"]["features"], serde_json::json!(["extract"]));
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
                let options = ServeOptions { max_runs: 1 };
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
        let (server_input, input) = io::pipe().unwrap();
        let server = thread::spawn({
            let captured = captured.clone();
            move || {
                serve_inner(
                    io::BufReader::new(server_input),
                    Box::new(captured),
                    &make,
                    &ServeOptions::default(),
                )
                .unwrap();
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
}
