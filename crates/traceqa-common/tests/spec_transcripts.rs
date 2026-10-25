//! Every message in `spec/transcripts/` must be a valid JSON-RPC line whose params,
//! result or error data parse as the types the client protocol names for its method.
//! The clients replay these conversations, so they have to stay true to the spec.

// Integration tests: a failed unwrap is the failure (clippy.toml covers only #[test] functions and test modules).
#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde_json::Value;
use traceqa_common::jsonrpc::{self, Message};

include!(concat!(env!("OUT_DIR"), "/spec_parse_as.rs"));

fn spec() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../spec"))
}

fn check(type_name: &str, value: &Value, at: &str) {
    parse_as(type_name, &value.to_string())
        .unwrap_or_else(|| panic!("{at}: no spec type called {type_name}"))
        .unwrap_or_else(|e| panic!("{at}: not a valid {type_name}: {e}"));
}

#[test]
fn transcripts_follow_the_client_protocol() {
    let contract: Value =
        serde_json::from_str(&fs::read_to_string(spec().join("client-protocol.schema.json")).unwrap()).unwrap();
    let contract = &contract["x-traceqa"];
    let type_of = |kind: &str, method: &str, part: &str| -> String {
        contract[kind][method][part]
            .as_str()
            .unwrap_or_else(|| panic!("{method} is not a client protocol {kind}"))
            .to_string()
    };

    let mut checked = 0;
    for entry in fs::read_dir(spec().join("transcripts")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|extension| extension != "jsonl") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let mut methods_by_id = HashMap::new();
        for (index, line) in fs::read_to_string(&path).unwrap().lines().enumerate() {
            let at = format!("{name}:{}", index + 1);
            let entry: Value = serde_json::from_str(line).unwrap_or_else(|e| panic!("{at}: {e}"));
            let Some(message) = entry.get("client").or_else(|| entry.get("engine")) else {
                continue;
            };
            match jsonrpc::decode(&message.to_string()).unwrap_or_else(|e| panic!("{at}: {e}")) {
                Message::Request { id, method, params } => {
                    assert!(
                        entry.get("client").is_some(),
                        "{at}: the engine never sends requests to a client"
                    );
                    check(&type_of("methods", &method, "params"), &params, &at);
                    methods_by_id.insert(id, method);
                }
                Message::Notification { method, params } => {
                    check(&type_of("notifications", &method, "params"), &params, &at);
                }
                Message::Response { id, result } => {
                    let method = methods_by_id
                        .get(&id)
                        .unwrap_or_else(|| panic!("{at}: answers no request"));
                    match result {
                        Ok(result) => check(&type_of("methods", method, "result"), &result, &at),
                        Err(error) => check("RpcErrorData", &error.data.unwrap_or_default(), &at),
                    }
                }
            }
            checked += 1;
        }
    }
    assert!(checked > 0, "no transcripts found");
}
