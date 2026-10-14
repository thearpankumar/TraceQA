//! The engine's side of the client protocol: `traceqa serve`.
//!
//! A client writes requests to our input, one per line. We write responses and
//! notifications to our output. A run executes on its own thread, so the client
//! can cancel it or start another while it is going.

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use serde::Serialize;
use serde_json::Value;
use traceqa_protocol::contracts::client_protocol::{method, notification, VERSION as PROTOCOL_VERSION};
use traceqa_protocol::jsonrpc::{self, code, Id, Message, RpcError};
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
    let data = RpcErrorData {
        kind: kind.to_string(),
        retryable: false,
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

/// A run that was started and may still be going.
struct ActiveRun {
    cancel: Arc<AtomicBool>,
    thread: JoinHandle<()>,
    /// Held until the client has been told the run was accepted. The run waits for it,
    /// so its first event can never overtake that answer.
    go: Option<Sender<()>>,
}

/// Serves clients until `input` ends, then waits for running runs to finish.
pub fn serve(input: impl BufRead, output: Box<dyn Write + Send>, drivers: Vec<DriverCommand>) -> io::Result<()> {
    let output: Output = Arc::new(Mutex::new(output));
    let mut runs: HashMap<String, ActiveRun> = HashMap::new();

    let mut failure_reading = None;
    for line in input.lines() {
        let line = match line {
            Ok(line) => line,
            // A line that is not UTF-8 is a bad request, not a reason to stop serving.
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                refuse_unreadable(&output, error.to_string());
                continue;
            }
            Err(error) => {
                failure_reading = Some(error);
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let (id, method, params) = match jsonrpc::decode(&line) {
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
            method::RUN_START => parse(params).and_then(|request| start_run(request, &drivers, &output, &mut runs)),
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
    drivers: &[DriverCommand],
    output: &Output,
    runs: &mut HashMap<String, ActiveRun>,
) -> Result<Value, RpcError> {
    let run = Run::prepare(request).map_err(|e| failure(code::INVALID_PARAMS, "invalid_params", e.to_string()))?;
    let run_id = run.id().to_string();
    let accepted = RunStartResult {
        run_id: run_id.clone(),
        run_folder: run.folder().to_string_lossy().into_owned(),
    };

    let cancel = Arc::new(AtomicBool::new(false));
    let (go, accepted_was_sent) = mpsc::channel::<()>();
    let thread = thread::spawn({
        let (run_id, cancel, output, commands) = (
            run_id.clone(),
            Arc::clone(&cancel),
            Arc::clone(output),
            drivers.to_vec(),
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
                drivers: &|| Drivers::new(commands.clone()),
                cancel: &cancel,
            };
            let manifest = run.execute(&execution, &mut on_event);
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
}
