//! The TraceQA MCP server: `traceqa mcp`.
//!
//! The Model Context Protocol lets an AI tool (Claude Code, Claude Desktop, Cursor and
//! others) start a program and call the tools it offers. The host writes JSON-RPC 2.0
//! requests to our input, one per line, and we answer on our output.
//!
//! The tools only call the engine and read run folders. They hold no logic of their own,
//! so an AI gets exactly what the command line and the SDKs get.

mod tools;

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use serde_json::{json, Value};
use traceqa_engine::driver::DriverCommand;
use traceqa_protocol::jsonrpc::{self, code, Id, Message, RpcError};

/// MCP versions this server speaks, newest first. A client asking for one of these gets it;
/// any other client is offered the newest and decides for itself.
pub const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

pub struct Config {
    /// Where runs are written and read. Absolute, so the answers name real places.
    pub runs_dir: PathBuf,
    pub drivers: Vec<DriverCommand>,
}

type Output = Arc<Mutex<Box<dyn Write + Send>>>;

fn send(output: &Output, message: &Message) {
    let mut output = output.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    // A host that has gone away cannot be told anything, so a failed write is dropped.
    let _ = writeln!(output, "{}", jsonrpc::encode(message)).and_then(|()| output.flush());
}

fn error(rpc_code: i64, message: impl Into<String>) -> RpcError {
    RpcError {
        code: rpc_code,
        message: message.into(),
        data: None,
    }
}

/// Tool calls still running, so that a `notifications/cancelled` can reach them.
type Running = Arc<Mutex<HashMap<Id, Arc<AtomicBool>>>>;

/// Serves one host until `input` ends. Tool calls still running are then cancelled and
/// waited for, so no run folder is left half written.
pub fn serve(input: impl BufRead, output: Box<dyn Write + Send>, config: Config) -> io::Result<()> {
    let output: Output = Arc::new(Mutex::new(output));
    let config = Arc::new(config);
    let running: Running = Arc::default();
    let mut calls: Vec<JoinHandle<()>> = Vec::new();

    let mut failure_reading = None;
    for line in input.lines() {
        let line = match line {
            Ok(line) => line,
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                let result = Err(error(code::PARSE_ERROR, e.to_string()));
                send(&output, &Message::Response { id: Id::Null, result });
                continue;
            }
            Err(e) => {
                failure_reading = Some(e);
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        calls.retain(|call| !call.is_finished());
        match jsonrpc::decode(&line) {
            Err(e) => {
                let result = Err(error(code::PARSE_ERROR, e.to_string()));
                send(&output, &Message::Response { id: Id::Null, result });
            }
            Ok(Message::Notification { method, params }) => {
                if method == "notifications/cancelled" {
                    cancel(&running, &params);
                }
                // notifications/initialized and others need nothing from us.
            }
            // We never send requests, so there is nothing a response could answer.
            Ok(Message::Response { .. }) => {}
            Ok(Message::Request { id, method, params }) => {
                let result = match method.as_str() {
                    "initialize" => Ok(initialize(&params)),
                    "ping" => Ok(json!({})),
                    "tools/list" => Ok(json!({ "tools": tools::list() })),
                    "tools/call" => {
                        calls.push(start_call(id, params, &output, &config, &running));
                        continue;
                    }
                    other => Err(error(code::METHOD_NOT_FOUND, format!("unknown method {other}"))),
                };
                send(&output, &Message::Response { id, result });
            }
        }
    }

    for flag in running.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).values() {
        flag.store(true, Ordering::Relaxed);
    }
    for call in calls {
        let _ = call.join();
    }
    failure_reading.map_or(Ok(()), Err)
}

fn initialize(params: &Value) -> Value {
    let asked = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let version = PROTOCOL_VERSIONS
        .iter()
        .find(|known| **known == asked)
        .unwrap_or(&PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "traceqa", "version": traceqa_engine::VERSION },
        "instructions": "TraceQA opens web pages in real browsers and lists every interactive component \
            (buttons, links, inputs) with stable ids that are the same in every browser. Start with \
            extract, then read the results with get_components. Ask for crops: true when you will \
            want to see components with get_crop.",
    })
}

fn cancel(running: &Running, params: &Value) {
    let Some(id) = params.get("requestId").cloned() else {
        return;
    };
    let Ok(id) = serde_json::from_value::<Id>(id) else {
        return;
    };
    if let Some(flag) = running.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&id) {
        flag.store(true, Ordering::Relaxed);
    }
}

/// Runs one tool call on its own thread, so the host can ping or cancel while it works.
fn start_call(id: Id, params: Value, output: &Output, config: &Arc<Config>, running: &Running) -> JoinHandle<()> {
    let cancelled = Arc::new(AtomicBool::new(false));
    running
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(id.clone(), Arc::clone(&cancelled));
    let (output, config, running) = (Arc::clone(output), Arc::clone(config), Arc::clone(running));
    thread::spawn(move || {
        let progress_token = params.pointer("/_meta/progressToken").cloned();
        let report = |progress: u64, message: &str| {
            let Some(token) = &progress_token else { return };
            let params = json!({ "progressToken": token, "progress": progress, "message": message });
            send(
                &output,
                &Message::Notification {
                    method: "notifications/progress".into(),
                    params,
                },
            );
        };
        let call = tools::Call {
            config: &config,
            cancelled: &cancelled,
            report: &report,
        };
        let result = call.run(&params);
        running
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&id);
        // A cancelled request is not answered: the host has already stopped waiting for it.
        if !cancelled.load(Ordering::Relaxed) {
            send(&output, &Message::Response { id, result });
        }
    })
}

#[cfg(test)]
mod tests;
