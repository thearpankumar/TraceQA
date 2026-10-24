//! The engine's side of the client protocol: `traceqa serve`.
//!
//! A client writes requests to our input, one per line. We write responses and
//! notifications to our output. A run executes on its own thread, so the client
//! can cancel it or start another while it is going.

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use serde::Serialize;
use serde_json::Value;
use traceqa_protocol::contracts::client_protocol::{method, notification, VERSION as PROTOCOL_VERSION};
use traceqa_protocol::jsonrpc::{self, code, Id, Line, Message, RpcError, MAX_REQUEST_LINE};
use traceqa_protocol::types::{
    EngineHelloParams, EngineHelloResult, Event, RpcErrorData, RunCancelParams, RunDoneParams, RunEventParams,
    RunStartParams, RunStartResult,
};

use crate::driver::{DriverCommand, Drivers};
use crate::run::{Execution, Run, FEATURES};

type Output = Arc<Mutex<Box<dyn Write + Send>>>;

fn send(output: &Output, message: &Message) {
    let mut output = output.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    // A client that has gone away cannot be told anything, so a failed write is dropped.
    let _ = writeln!(output, "{}", jsonrpc::encode(message)).and_then(|()| output.flush());
}

fn notify(output: &Output, method: &str, params: &impl Serialize) {
    let params = serde_json::to_value(params).expect("spec types always serialise");
    send(
        output,
        &Message::Notification {
            method: method.to_string(),
            params,
        },
    );
}

fn failure(rpc_code: i64, kind: &str, message: impl Into<String>) -> RpcError {
    failure_with(rpc_code, kind, false, message)
}

fn failure_with(rpc_code: i64, kind: &str, retryable: bool, message: impl Into<String>) -> RpcError {
    let data = RpcErrorData {
        kind: kind.to_string(),
        retryable,
    };
    RpcError {
        code: rpc_code,
        message: message.into(),
        data: serde_json::to_value(data).ok(),
    }
}

fn parse<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    serde_json::from_value(params).map_err(|e| failure(code::INVALID_PARAMS, "invalid_params", e.to_string()))
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
    mut input: impl BufRead,
    output: Box<dyn Write + Send>,
    make_drivers: &MakeDrivers,
    options: &ServeOptions,
) -> io::Result<()> {
    let output: Output = Arc::new(Mutex::new(output));
    let mut runs: HashMap<String, ActiveRun> = HashMap::new();
    let active = Arc::new(AtomicUsize::new(0));
    let max_runs = options.max_runs.max(1);

    let mut failure_reading = None;
    let mut buf = Vec::new();
    loop {
        // Lines are read with a limit: a client that never sends a newline cannot use all the memory.
        match jsonrpc::read_line_bounded(&mut input, MAX_REQUEST_LINE, &mut buf) {
            Ok(Line::Eof) => break,
            Ok(Line::Complete) => {}
            Ok(Line::TooLong) => {
                let message = format!(
                    "request larger than {} MiB was skipped",
                    MAX_REQUEST_LINE / (1024 * 1024)
                );
                let result = Err(failure(code::INVALID_REQUEST, "invalid_request", message));
                send(&output, &Message::Response { id: Id::Null, result });
                continue;
            }
            Err(error) => {
                failure_reading = Some(error);
                break;
            }
        }
        // A line that is not UTF-8 is a bad request, not a reason to stop serving.
        let line = match std::str::from_utf8(&buf) {
            Ok(line) => line,
            Err(error) => {
                refuse_unreadable(&output, format!("the request is not valid UTF-8: {error}"));
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let (id, method, params) = match jsonrpc::decode(line) {
            Ok(Message::Request { id, method, params }) => (id, method, params),
            Ok(_) => continue,
            Err(error) => {
                refuse_unreadable(&output, error.to_string());
                continue;
            }
        };

        // Forget runs that have finished, so an engine that serves for a long time does not grow.
        runs.retain(|_, run| !run.thread.is_finished());

        let result = match method.as_str() {
            method::ENGINE_HELLO => parse(params).and_then(hello),
            method::RUN_START => parse(params)
                .and_then(|request| start_run(request, make_drivers, &output, &mut runs, &active, max_runs)),
            method::RUN_CANCEL => parse(params).and_then(|request: RunCancelParams| match runs.get(&request.run_id) {
                Some(run) => {
                    run.cancel.store(true, Ordering::Relaxed);
                    answer(serde_json::json!({}))
                }
                None => Err(failure(
                    code::INVALID_PARAMS,
                    "invalid_params",
                    format!("no running run {}", request.run_id),
                )),
            }),
            other => Err(failure(
                code::METHOD_NOT_FOUND,
                "unsupported",
                format!("unknown method {other}"),
            )),
        };
        send(&output, &Message::Response { id, result });
        for run in runs.values_mut() {
            run.go.take();
        }
    }

    // Runs are always allowed to finish, so no run folder is left half written.
    for run in runs.into_values() {
        drop(run.go);
        let _ = run.thread.join();
    }
    failure_reading.map_or(Ok(()), Err)
}

/// Answers a line that could not be understood. Its id is unknown, so the reply carries none.
fn refuse_unreadable(output: &Output, reason: String) {
    let result = Err(failure(code::PARSE_ERROR, "invalid_request", reason));
    send(output, &Message::Response { id: Id::Null, result });
}

fn hello(hello: EngineHelloParams) -> Result<Value, RpcError> {
    if hello.protocol_version != PROTOCOL_VERSION {
        return Err(failure(
            code::APPLICATION_ERROR,
            "version_mismatch",
            format!(
                "engine speaks client protocol {PROTOCOL_VERSION}, client asked for {}",
                hello.protocol_version
            ),
        ));
    }
    answer(EngineHelloResult {
        engine_version: crate::VERSION.to_string(),
        protocol_version: PROTOCOL_VERSION,
        features: FEATURES.iter().map(|feature| feature.to_string()).collect(),
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
            "busy",
            true,
            format!("busy: {max_runs} runs are already going. Wait for one to finish, then try again."),
        ));
    }
    let counted = RunSlot(Arc::clone(active));
    let run = Run::prepare(request).map_err(|e| failure(code::INVALID_PARAMS, "invalid_params", e.to_string()))?;
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
            Arc::clone(output),
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
                notify(&output, notification::RUN_EVENT, &params);
            };
            let execution = Execution {
                drivers: &|| make_drivers(),
                cancel: &cancel,
                policy: None,
            };
            let manifest = run.execute(&execution, &mut on_event);
            drop(counted);
            let done = RunDoneParams {
                run_id: run_id.clone(),
                status: manifest.status,
                manifest,
            };
            notify(&output, notification::RUN_DONE, &done);
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
        let replies = converse(&[request(1, "run.start", start)]);

        // The response comes first and only says the run was accepted.
        let run_id = replies[0]["result"]["runId"].as_str().unwrap();
        assert_eq!(replies[0]["id"], 1);
        // Everything after it is a notification about that run, ending with run.done.
        let methods: Vec<&str> = replies[1..]
            .iter()
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
                Err(DriverError {
                    kind: "unsupported".into(),
                    message: "scripted: no browser".into(),
                })
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
}
