//! MCP resources: runs, their components and their crops, addressed as `traceqa://runs/...`.
//!
//! The same data the tools return, for hosts that let a person or a model attach a resource.
//! Every segment of a URI is checked as a plain name and every file is read through
//! `safe_file_in`, so a URI cannot reach anything outside the runs folder. JSON goes through the
//! same cleaning, flagging and size cap as a tool answer. There are no subscriptions and no
//! list-changed notices: a host reads again when it wants to.

use std::fs;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use traceqa_protocol::jsonrpc::{code, RpcError};
use traceqa_protocol::types::RunManifest;

use crate::tools::{clean, plain_name, public_message, read_json, render, Cut, ToolError};
use crate::Config;

const SCHEME: &str = "traceqa://runs/";
/// The most runs `resources/list` shows. Run ids start with their date, so the newest are found
/// from the folder names alone, without opening every run.
const MAX_LISTED: usize = 50;
/// The most folders `resources/list` looks into to find [`MAX_LISTED`] runs.
const MAX_SCANNED: usize = 500;
/// The "resource not found" code of the MCP specification.
const NOT_FOUND: i64 = -32002;

pub(crate) fn run_uri(run_id: &str) -> String {
    format!("{SCHEME}{run_id}")
}

pub(crate) fn components_uri(run_id: &str, target: &str) -> String {
    format!("{SCHEME}{run_id}/{target}/components")
}

pub(crate) fn crop_uri(run_id: &str, target: &str, component_id: &str) -> String {
    format!("{SCHEME}{run_id}/{target}/crops/{component_id}")
}

fn assistant_only() -> Value {
    json!({ "audience": ["assistant"] })
}

/// The links `extract` adds to its answer: the run, and the components of each target that has them.
pub(crate) fn extract_links(runs_dir: &std::path::Path, manifest: &RunManifest) -> Vec<Value> {
    let run = &manifest.run_id;
    let mut links = vec![json!({
        "type": "resource_link", "uri": run_uri(run), "name": run, "mimeType": "application/json",
        "description": "The run's manifest.", "annotations": assistant_only()
    })];
    for target in &manifest.targets {
        if runs_dir.join(run).join(&target.dir).join("components.json").is_file() {
            links.push(json!({
                "type": "resource_link", "uri": components_uri(run, &target.dir),
                "name": format!("{run}/{}", target.dir), "mimeType": "application/json",
                "description": "Every component found in this browser and viewport.",
                "annotations": assistant_only()
            }));
        }
    }
    links
}

/// `resources/list`: the newest runs.
pub(crate) fn list(config: &Config) -> Value {
    let mut names: Vec<String> = fs::read_dir(&config.runs_dir)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.starts_with("r_") && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        .collect();
    names.sort_unstable_by(|a, b| b.cmp(a));
    let resources: Vec<Value> = names
        .iter()
        .take(MAX_SCANNED)
        .filter_map(|name| {
            let manifest: RunManifest = read_json(&config.runs_dir.join(name).join("run.json"))?;
            let url: String = clean(&manifest.url).chars().take(120).collect();
            Some(json!({
                "uri": run_uri(name),
                "name": name,
                "title": format!("{url} ({})", json!(manifest.status).as_str().unwrap_or_default()),
                "description": "A TraceQA run: its page, status and browser targets.",
                "mimeType": "application/json",
                "annotations": assistant_only(),
            }))
        })
        .take(MAX_LISTED)
        .collect();
    json!({ "resources": resources })
}

/// `resources/templates/list`.
pub(crate) fn templates() -> Value {
    let template = |uri: &str, name: &str, description: &str, mime: &str| {
        json!({
            "uriTemplate": uri, "name": name, "description": description, "mimeType": mime,
            "annotations": assistant_only()
        })
    };
    json!({ "resourceTemplates": [
        template("traceqa://runs/{run_id}", "run", "A run's manifest: page, status and targets.", "application/json"),
        template(
            "traceqa://runs/{run_id}/{target}/components",
            "components",
            "Every component one browser found at one viewport, for example target chromium-1280x800.",
            "application/json",
        ),
        template(
            "traceqa://runs/{run_id}/{target}/crops/{component_id}",
            "crop",
            "The picture of one component. Only for runs made with crops: true.",
            "image/png",
        ),
    ] })
}

fn rpc(rpc_code: i64, message: impl Into<String>) -> RpcError {
    RpcError {
        code: rpc_code,
        message: message.into(),
        data: None,
    }
}

/// A tool error as the JSON-RPC error a resource read answers with.
fn failed(config: &Config, why: &ToolError) -> RpcError {
    let message = clean(&public_message(&why.to_string(), &config.runs_dir));
    let message: String = message.chars().take(500).collect();
    match why {
        ToolError::NotPlainName { .. } => rpc(code::INVALID_PARAMS, message),
        _ => rpc(NOT_FOUND, message),
    }
}

/// `resources/read`.
pub(crate) fn read(config: &Config, params: &Value) -> Result<Value, RpcError> {
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| rpc(code::INVALID_PARAMS, "resources/read needs a uri"))?;
    let unknown = || {
        let shown: String = clean(uri).chars().take(100).collect();
        rpc(
            code::INVALID_PARAMS,
            format!("{shown:?} is not a TraceQA resource. resources/templates/list shows the forms."),
        )
    };
    let rest = uri.strip_prefix(SCHEME).ok_or_else(unknown)?;
    let segments: Vec<&str> = rest.split('/').collect();
    for (segment, what) in segments.iter().zip(["run_id", "target", "kind", "component_id"]) {
        // The kind is a fixed word, checked below; the others name folders or components.
        if what != "kind" {
            plain_name(segment, what).map_err(|why| failed(config, &why))?;
        }
    }
    let fail = |why: ToolError| failed(config, &why);
    match segments.as_slice() {
        [run] => {
            let mut manifest = config.manifest(run).map_err(fail)?;
            for target in &mut manifest.targets {
                target.error = target.error.take().map(|e| public_message(&e, &config.runs_dir));
            }
            let total = manifest.targets.len();
            let value = serde_json::to_value(&manifest).expect("plain data always serialises");
            let cut = Cut::whole("targets", "targets_shown", total, "The other targets are not returned");
            Ok(json_contents(uri, value, cut))
        }
        [run, target, "components"] => {
            let (_, file) = config.components_file(run, Some(target)).map_err(fail)?;
            let total = file.components.len();
            let value = serde_json::to_value(&file).expect("plain data always serialises");
            let cut = Cut::whole(
                "components",
                "shown",
                total,
                "Use find or get_components with offset to read the rest",
            );
            Ok(json_contents(uri, value, cut))
        }
        [run, target, "crops", component] => {
            let crop = config.read_crop(run, Some(target), component).map_err(fail)?;
            Ok(json!({ "contents": [{
                "uri": uri, "mimeType": "image/png", "blob": STANDARD.encode(&crop.bytes)
            }] }))
        }
        _ => Err(unknown()),
    }
}

fn json_contents(uri: &str, value: Value, cut: Cut) -> Value {
    let (text, _) = render(value, Some(cut));
    json!({ "contents": [{ "uri": uri, "mimeType": "application/json", "text": text }] })
}
