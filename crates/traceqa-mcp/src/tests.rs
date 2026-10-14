use std::io::{BufRead, BufReader, PipeReader, PipeWriter, Write};
use std::path::{Path, PathBuf};
use std::{fs, process};

use serde_json::{json, Value};
use traceqa_engine::driver::DriverCommand;

use super::*;
use crate::tools::base64;

const EXAMPLES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../spec/examples");
const RUN_ID: &str = "r_20261013_210455_a1b2";
const COMPONENT: &str = "c_9f2a41c07b3e";
const PICTURE: &[u8] = b"\x89PNG not really";

/// A host talking to a server over real pipes, as Claude Code would.
struct Host {
    input: Option<PipeWriter>,
    output: BufReader<PipeReader>,
    server: Option<JoinHandle<()>>,
    next_id: u64,
    /// Notifications seen while waiting for answers.
    notes: Vec<Value>,
}

impl Host {
    fn start(runs_dir: PathBuf) -> Self {
        let (server_input, input) = io::pipe().unwrap();
        let (output, server_output) = io::pipe().unwrap();
        // No such driver exists, so a run fails its targets without opening a browser.
        let drivers = vec![DriverCommand::parse("traceqa-no-such-driver").unwrap()];
        let server = thread::spawn(move || {
            let config = Config { runs_dir, drivers };
            serve(BufReader::new(server_input), Box::new(server_output), config).unwrap();
        });
        Self {
            input: Some(input),
            output: BufReader::new(output),
            server: Some(server),
            next_id: 0,
            notes: Vec::new(),
        }
    }

    fn write(&mut self, message: Value) {
        writeln!(self.input.as_mut().unwrap(), "{message}").unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.write(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let mut line = String::new();
            assert!(self.output.read_line(&mut line).unwrap() > 0, "the server stopped");
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["id"] == id {
                return message;
            }
            self.notes.push(message);
        }
    }

    fn call(&mut self, tool: &str, arguments: Value) -> Value {
        let answer = self.request("tools/call", json!({ "name": tool, "arguments": arguments }));
        answer["result"].clone()
    }

    fn stop(mut self) {
        drop(self.input.take());
        self.server.take().unwrap().join().unwrap();
    }
}

fn text_of(result: &Value) -> Value {
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

/// A runs folder holding one finished run, made from the spec examples.
fn runs_folder(name: &str) -> PathBuf {
    let runs = std::env::temp_dir().join(format!("traceqa-mcp-{name}-{}", process::id()));
    let _ = fs::remove_dir_all(&runs);
    let target = runs.join(RUN_ID).join("firefox-1280x800");
    fs::create_dir_all(target.join("crops")).unwrap();
    let copy = |example: &str, to: &Path| fs::copy(Path::new(EXAMPLES).join(example), to).unwrap();
    copy("RunManifest.completed.json", &runs.join(RUN_ID).join("run.json"));
    copy("ComponentsFile.shop.json", &target.join("components.json"));
    fs::write(target.join("crops").join(format!("{COMPONENT}.png")), PICTURE).unwrap();
    runs
}

#[test]
fn says_hello_in_the_hosts_version_when_it_knows_it() {
    let mut host = Host::start(runs_folder("hello"));
    let answer = host.request(
        "initialize",
        json!({ "protocolVersion": "2025-06-18", "capabilities": {} }),
    );
    assert_eq!(answer["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(answer["result"]["serverInfo"]["name"], "traceqa");
    assert!(answer["result"]["capabilities"]["tools"].is_object());

    let unknown = host.request("initialize", json!({ "protocolVersion": "1999-01-01" }));
    assert_eq!(unknown["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);

    host.write(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    assert_eq!(host.request("ping", json!({}))["result"], json!({}));
    host.stop();
}

#[test]
fn lists_four_tools_each_with_a_schema() {
    let mut host = Host::start(runs_folder("list"));
    let tools = host.request("tools/list", json!({}))["result"]["tools"].clone();
    let names: Vec<&str> = tools
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["extract", "list_runs", "get_components", "get_crop"]);
    for tool in tools.as_array().unwrap() {
        assert_eq!(tool["inputSchema"]["type"], "object", "{}", tool["name"]);
        assert!(tool["description"].as_str().unwrap().len() > 40);
    }
    host.stop();
}

#[test]
fn refuses_unknown_methods_and_tools_as_protocol_errors() {
    let mut host = Host::start(runs_folder("unknown"));
    assert_eq!(host.request("resources/list", json!({}))["error"]["code"], -32601);
    assert_eq!(
        host.request("tools/call", json!({ "name": "explode" }))["error"]["code"],
        -32602
    );
    host.stop();
}

#[test]
fn reads_a_run_back() {
    let runs = runs_folder("read");
    let mut host = Host::start(runs.clone());

    let listed = text_of(&host.call("list_runs", json!({})));
    assert_eq!(listed["runs"][0]["run_id"], RUN_ID);
    assert_eq!(listed["runs"][0]["targets"][0]["target"], "firefox-1280x800");

    // The run has one target, so it may be left out.
    let summary = host.call("get_components", json!({ "run_id": RUN_ID }));
    assert_eq!(summary["isError"], false);
    let summary = text_of(&summary);
    assert_eq!(summary["components"][0]["id"], COMPONENT);
    assert_eq!(summary["components"][0]["instances"], 2);
    assert_eq!(summary["components"][0]["has_crop"], true);

    let filtered = text_of(&host.call("get_components", json!({ "run_id": RUN_ID, "role": "link" })));
    assert_eq!(
        (filtered["total"].clone(), filtered["matching"].clone()),
        (json!(1), json!(0))
    );
    let by_name = text_of(&host.call("get_components", json!({ "run_id": RUN_ID, "name": "CART" })));
    assert_eq!(by_name["matching"], 1);

    let full = text_of(&host.call("get_components", json!({ "run_id": RUN_ID, "component_id": COMPONENT })));
    assert_eq!(full["instances"].as_array().unwrap().len(), 2);

    let crop = host.call("get_crop", json!({ "run_id": RUN_ID, "component_id": COMPONENT }));
    assert_eq!(crop["content"][1]["type"], "image");
    assert_eq!(crop["content"][1]["mimeType"], "image/png");
    assert_eq!(crop["content"][1]["data"], base64(PICTURE));
    host.stop();
    fs::remove_dir_all(runs).unwrap();
}

#[test]
fn mistakes_come_back_as_tool_errors_the_ai_can_read() {
    let runs = runs_folder("mistakes");
    // A state file must never be echoed back: it holds session keys.
    let bad_state = runs.join("bad-state.json");
    fs::write(&bad_state, r#"{"token": "secret-token-123"}"#).unwrap();
    let mut host = Host::start(runs.clone());
    let failing = [
        ("get_components", json!({ "run_id": "../../etc" }), "plain name"),
        ("get_components", json!({ "run_id": "r_missing" }), "no run r_missing"),
        (
            "get_components",
            json!({ "run_id": RUN_ID, "target": "webkit-1x1" }),
            "has no target",
        ),
        (
            "get_components",
            json!({ "run_id": RUN_ID, "component_id": "c_nope" }),
            "no component",
        ),
        ("get_crop", json!({ "run_id": RUN_ID }), "missing field `component_id`"),
        (
            "extract",
            json!({ "url": "https://x.test", "colour": "red" }),
            "unknown field `colour`",
        ),
        (
            "extract",
            json!({ "url": "https://x.test", "viewports": ["big"] }),
            "WIDTHxHEIGHT",
        ),
        ("extract", json!({ "url": "not-a-url" }), "not-a-url"),
        (
            "extract",
            json!({ "url": "https://x.test", "storage_state": "/no/such/state.json" }),
            "could not read the state file",
        ),
        (
            "extract",
            json!({ "url": "https://x.test", "storage_state": bad_state.display().to_string() }),
            "is not a storage state file",
        ),
    ];
    for (tool, arguments, expected) in failing {
        let result = host.call(tool, arguments.clone());
        let text = result["content"][0]["text"].as_str().unwrap();
        assert_eq!(result["isError"], true, "{tool} {arguments}");
        assert!(text.contains(expected), "{tool} {arguments}: {text}");
        assert!(
            !text.contains("secret-token-123"),
            "a state file's contents were shown: {text}"
        );
    }
    host.stop();
    fs::remove_dir_all(runs).unwrap();
}

#[test]
fn extract_reports_progress_and_a_summary() {
    let runs = runs_folder("extract");
    let mut host = Host::start(runs.clone());
    let params = json!({
        "name": "extract",
        "arguments": { "url": "https://example.test/", "viewports": ["1280x800", "390x844"] },
        "_meta": { "progressToken": "p1" }
    });
    let answer = host.request("tools/call", params);
    let summary = text_of(&answer["result"]);
    // No driver can open chromium here, so both targets fail, and the call itself succeeds.
    assert_eq!(answer["result"]["isError"], false);
    assert_eq!(summary["status"], "failed");
    let targets: Vec<&str> = summary["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["target"].as_str().unwrap())
        .collect();
    assert_eq!(targets, ["chromium-1280x800", "chromium-390x844"]);
    assert!(runs
        .join(summary["run_id"].as_str().unwrap())
        .join("run.json")
        .is_file());

    let progress: Vec<&Value> = host
        .notes
        .iter()
        .filter(|n| n["method"] == "notifications/progress")
        .collect();
    assert!(!progress.is_empty());
    assert!(progress.iter().all(|n| n["params"]["progressToken"] == "p1"));
    let steps: Vec<u64> = progress
        .iter()
        .map(|n| n["params"]["progress"].as_u64().unwrap())
        .collect();
    assert!(
        steps.windows(2).all(|pair| pair[0] < pair[1]),
        "progress must increase: {steps:?}"
    );
    host.stop();
    fs::remove_dir_all(runs).unwrap();
}

#[test]
fn a_cancel_reaches_the_call_it_names() {
    let running: Running = Arc::default();
    let flag = Arc::new(AtomicBool::new(false));
    running.lock().unwrap().insert(Id::Number(7), Arc::clone(&flag));
    cancel(&running, &json!({ "requestId": 8 }));
    assert!(!flag.load(Ordering::Relaxed));
    cancel(&running, &json!({ "requestId": 7, "reason": "user stopped it" }));
    assert!(flag.load(Ordering::Relaxed));
}

#[test]
fn base64_matches_the_standard() {
    let cases = [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foobar", "Zm9vYmFy"),
    ];
    for (plain, encoded) in cases {
        assert_eq!(base64(plain.as_bytes()), encoded);
    }
    assert_eq!(base64(&[0xff, 0xfe, 0x00]), "//4A");
}
