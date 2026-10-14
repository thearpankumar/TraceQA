//! The tools an AI can call. Each one checks its arguments, calls the engine or reads a run
//! folder, and answers with text (JSON) or a picture.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use traceqa_engine::driver::Drivers;
use traceqa_engine::{Execution, Run};
use traceqa_protocol::jsonrpc::{code, RpcError};
use traceqa_protocol::types::{
    BrowserTarget, Component, ComponentsFile, Event, ExtractOptions, RunManifest, RunStartParams, Viewport,
};

use crate::Config;

/// What `tools/list` returns. The descriptions are what the AI reads to choose a tool.
pub fn list() -> Value {
    json!([
        {
            "name": "extract",
            "description": "Open a web page in real browsers and list every interactive component on it: \
                buttons, links, inputs, selects and anything else that can be clicked or typed into. \
                Each browser is run at each viewport. Components get ids that are the same in every browser, \
                so results can be compared. Reads the page as it is; it does not click or type. \
                Takes from a few seconds to a minute per browser and viewport. \
                Returns a run id and a summary per browser and viewport; read the details with get_components.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "The page to read, for example https://example.com." },
                    "browsers": {
                        "type": "array", "items": { "type": "string" },
                        "description": "Default [\"chromium\"]. Known values: chromium, chrome, msedge, firefox, webkit, safari. \
                            cdp=ENDPOINT attaches to a running browser; webdriver=URL uses a WebDriver server."
                    },
                    "viewports": {
                        "type": "array", "items": { "type": "string" },
                        "description": "Screen sizes as WIDTHxHEIGHT. Default [\"1280x800\"]. A phone is 390x844."
                    },
                    "crops": {
                        "type": "boolean",
                        "description": "Also save a picture of each component, for get_crop. Slower on long pages. Default false."
                    },
                    "settle_ms": {
                        "type": "integer", "minimum": 0,
                        "description": "Extra time to wait after the page loads, for pages with a timed intro."
                    },
                    "max_scroll_steps": {
                        "type": "integer", "minimum": 0,
                        "description": "How many screens to scroll through. 0 reads the page without scrolling."
                    },
                    "timeout_ms": { "type": "integer", "minimum": 0, "description": "Time allowed for the page to load." },
                    "storage_state": {
                        "type": "string",
                        "description": "Path of a state file (Playwright's storageState format, as `traceqa login` saves) holding cookies and stored values, for pages behind a sign-in. Its contents are never shown."
                    }
                },
                "required": ["url"],
                "additionalProperties": false
            }
        },
        {
            "name": "list_runs",
            "description": "List earlier runs, newest first, with their page, status and targets.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "minimum": 1, "description": "At most this many. Default 20." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "get_components",
            "description": "Read the components one run found in one browser at one viewport. \
                Without component_id, returns a short line per component, which can be filtered. \
                With component_id, returns that component in full: every instance, its selectors, \
                position and state.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "run_id": { "type": "string" },
                    "target": {
                        "type": "string",
                        "description": "Browser and viewport, for example chromium-1280x800. May be left out when the run has one."
                    },
                    "component_id": { "type": "string", "description": "For example c_54751a251306." },
                    "role": { "type": "string", "description": "Only this role, for example button, link or textbox." },
                    "name": { "type": "string", "description": "Only names containing this text, ignoring case." },
                    "limit": { "type": "integer", "minimum": 1, "description": "At most this many components. Default 100." }
                },
                "required": ["run_id"],
                "additionalProperties": false
            }
        },
        {
            "name": "get_crop",
            "description": "Show the picture of one component, cut from a screenshot of the page. \
                Only for runs made with crops: true.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "run_id": { "type": "string" },
                    "target": { "type": "string", "description": "May be left out when the run has one." },
                    "component_id": { "type": "string" }
                },
                "required": ["run_id", "component_id"],
                "additionalProperties": false
            }
        }
    ])
}

/// One `tools/call`.
pub struct Call<'a> {
    pub config: &'a Config,
    pub cancelled: &'a AtomicBool,
    /// Reports progress to the host, when it asked for that.
    pub report: &'a (dyn Fn(u64, &str) + Sync),
}

/// A tool's answer: the content blocks MCP defines.
type Content = Vec<Value>;

fn text(value: &impl serde::Serialize) -> Value {
    let text = serde_json::to_string_pretty(value).expect("plain data always serialises");
    json!({ "type": "text", "text": text })
}

impl Call<'_> {
    /// A protocol error for a call the server cannot make sense of. A tool that runs and
    /// fails answers normally with `isError`, so the AI sees why and can try again.
    pub fn run(&self, params: &Value) -> Result<Value, RpcError> {
        let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
        let arguments = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
        let outcome = match name {
            "extract" => args(arguments).and_then(|a| self.extract(a)),
            "list_runs" => args(arguments).and_then(|a| self.list_runs(a)),
            "get_components" => args(arguments).and_then(|a| self.get_components(a)),
            "get_crop" => args(arguments).and_then(|a| self.get_crop(a)),
            other => {
                return Err(RpcError {
                    code: code::INVALID_PARAMS,
                    message: format!("unknown tool {other:?}"),
                    data: None,
                })
            }
        };
        Ok(match outcome {
            Ok(content) => json!({ "content": content, "isError": false }),
            Err(why) => json!({ "content": [{ "type": "text", "text": why }], "isError": true }),
        })
    }

    fn extract(&self, args: ExtractArgs) -> Result<Content, String> {
        let browsers = if args.browsers.is_empty() {
            vec!["chromium".to_string()]
        } else {
            args.browsers
        };
        let viewports = if args.viewports.is_empty() {
            vec!["1280x800".to_string()]
        } else {
            args.viewports
        };
        let mut targets = browsers
            .iter()
            .map(|b| BrowserTarget::parse(b))
            .collect::<Result<Vec<_>, _>>()?;
        for target in &mut targets {
            target.headless = Some(true);
        }
        let storage_state = match &args.storage_state {
            Some(path) => {
                let text =
                    fs::read_to_string(path).map_err(|e| format!("could not read the state file {path}: {e}"))?;
                // The contents are a secret, so a parse error names the file and not what is in it.
                Some(serde_json::from_str(&text).map_err(|_| format!("{path} is not a storage state file"))?)
            }
            None => None,
        };
        let request = RunStartParams {
            feature: "extract".into(),
            url: args.url,
            targets,
            viewports: viewports.iter().map(|v| Viewport::parse(v)).collect::<Result<_, _>>()?,
            out_dir: Some(self.config.runs_dir.display().to_string()),
            parallel: None,
            options: Some(ExtractOptions {
                crops: Some(args.crops),
                include_hidden: Some(true),
                max_scroll_steps: args.max_scroll_steps,
                timeout_ms: args.timeout_ms,
                settle_ms: args.settle_ms,
            }),
            storage_state,
        };
        let run = Run::prepare(request).map_err(|e| e.to_string())?;
        let commands = self.config.drivers.clone();
        let execution = Execution {
            drivers: &|| Drivers::new(commands.clone()),
            cancel: self.cancelled,
        };
        let report = self.report;
        let mut on_event = |event: &Event| report(event.seq + 1, &event.message);
        let manifest = run.execute(&execution, &mut on_event);

        let targets: Vec<Value> = manifest
            .targets
            .iter()
            .map(|t| {
                json!({
                    "target": t.dir,
                    "status": t.status,
                    "components": t.component_count,
                    "instances": t.instance_count,
                    "warnings": t.warning_count,
                    "seconds": (t.duration_ms as f64 / 100.0).round() / 10.0,
                    "error": t.error,
                })
            })
            .collect();
        Ok(vec![text(&json!({
            "run_id": manifest.run_id,
            "status": manifest.status,
            "url": manifest.url,
            "targets": targets,
            "next": "Read what was found with get_components, giving this run_id and a target.",
        }))])
    }

    fn list_runs(&self, args: ListRunsArgs) -> Result<Content, String> {
        let mut runs: Vec<RunManifest> = match fs::read_dir(&self.config.runs_dir) {
            Ok(entries) => entries
                .filter_map(|entry| read_json(&entry.ok()?.path().join("run.json")).ok())
                .collect(),
            Err(_) => Vec::new(), // no runs yet
        };
        runs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        runs.truncate(args.limit.unwrap_or(20));
        let runs: Vec<Value> = runs
            .iter()
            .map(|run| {
                json!({
                    "run_id": run.run_id,
                    "url": run.url,
                    "status": run.status,
                    "started_at": run.started_at,
                    "targets": run.targets.iter().map(|t| json!({
                        "target": t.dir, "status": t.status, "components": t.component_count
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        Ok(vec![text(&json!({ "runs_dir": self.config.runs_dir, "runs": runs }))])
    }

    fn get_components(&self, args: ComponentsArgs) -> Result<Content, String> {
        let (dir, file) = self.components_file(&args.run_id, args.target.as_deref())?;
        if let Some(id) = &args.component_id {
            return Ok(vec![text(find(&file, id)?)]);
        }
        let name = args.name.as_deref().map(str::to_lowercase);
        let matching: Vec<&Component> = file
            .components
            .iter()
            .filter(|c| args.role.as_deref().is_none_or(|role| c.role == role))
            .filter(|c| name.as_deref().is_none_or(|name| c.name.to_lowercase().contains(name)))
            .collect();
        let limit = args.limit.unwrap_or(100);
        let lines: Vec<Value> = matching
            .iter()
            .take(limit)
            .map(|c| {
                let first = c.instances.first();
                json!({
                    "id": c.id,
                    "role": c.role,
                    "name": c.name,
                    "instances": c.instances.len(),
                    "visible": c.instances.iter().filter(|i| i.state.visible).count(),
                    "path": c.path,
                    "selector": first.and_then(|i| i.selectors.first()),
                    "confidence": c.confidence,
                    "has_crop": c.crop.is_some(),
                })
            })
            .collect();
        Ok(vec![text(&json!({
            "target": dir,
            "url": file.url,
            "title": file.title,
            "browser": file.browser,
            "viewport": file.viewport,
            "total": file.components.len(),
            "matching": matching.len(),
            "shown": lines.len(),
            "components": lines,
            "warnings": file.warnings,
        }))])
    }

    fn get_crop(&self, args: CropArgs) -> Result<Content, String> {
        let (dir, file) = self.components_file(&args.run_id, args.target.as_deref())?;
        let component = find(&file, &args.component_id)?;
        let Some(crop) = &component.crop else {
            return Err(format!(
                "{} has no picture. Run extract again with crops: true to get pictures.",
                component.id
            ));
        };
        if Path::new(crop)
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err(format!("{} names a picture outside its folder", component.id));
        }
        let path = self.run_dir(&args.run_id)?.join(&dir).join(crop);
        let bytes = fs::read(&path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
        let label = if component.name.is_empty() {
            "(no name)"
        } else {
            &component.name
        };
        Ok(vec![
            json!({ "type": "text", "text": format!("{} {:?}, {}, in {dir}", component.role, label, component.id) }),
            json!({ "type": "image", "data": base64(&bytes), "mimeType": "image/png" }),
        ])
    }

    /// A run's folder. The id comes from the AI, so it must name a folder directly inside the runs folder.
    fn run_dir(&self, run_id: &str) -> Result<PathBuf, String> {
        plain_name(run_id, "run_id")?;
        let dir = self.config.runs_dir.join(run_id);
        if dir.join("run.json").is_file() {
            Ok(dir)
        } else {
            Err(format!(
                "no run {run_id} in {}. list_runs shows the runs there.",
                self.config.runs_dir.display()
            ))
        }
    }

    fn components_file(&self, run_id: &str, target: Option<&str>) -> Result<(String, ComponentsFile), String> {
        let dir = self.run_dir(run_id)?;
        let manifest: RunManifest = read_json(&dir.join("run.json"))?;
        let names: Vec<&str> = manifest.targets.iter().map(|t| t.dir.as_str()).collect();
        let target = match target {
            Some(target) if names.contains(&target) => target.to_string(),
            Some(target) => {
                return Err(format!(
                    "run {run_id} has no target {target}. It has: {}",
                    names.join(", ")
                ))
            }
            None if names.len() == 1 => names[0].to_string(),
            None => {
                return Err(format!(
                    "run {run_id} has several targets, give one of: {}",
                    names.join(", ")
                ))
            }
        };
        let summary = manifest
            .targets
            .iter()
            .find(|t| t.dir == target)
            .expect("checked above");
        let path = dir.join(&target).join("components.json");
        if !path.is_file() {
            let why = summary.error.as_deref().unwrap_or("it did not finish");
            return Err(format!("{target} found nothing: {why}"));
        }
        Ok((target, read_json(&path)?))
    }
}

fn find<'f>(file: &'f ComponentsFile, id: &str) -> Result<&'f Component, String> {
    file.components
        .iter()
        .find(|c| c.id == id)
        .ok_or_else(|| format!("no component {id} here. get_components lists them."))
}

/// A folder name with no way out of its parent.
fn plain_name(name: &str, what: &str) -> Result<(), String> {
    let plain = !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':']);
    if plain {
        Ok(())
    } else {
        Err(format!("{what} must be a plain name, got {name:?}"))
    }
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{} is not valid: {e}", path.display()))
}

/// Arguments with a message the AI can act on when they are wrong.
fn args<T: DeserializeOwned>(arguments: Value) -> Result<T, String> {
    serde_json::from_value(arguments).map_err(|e| format!("invalid arguments: {e}"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExtractArgs {
    url: String,
    #[serde(default)]
    browsers: Vec<String>,
    #[serde(default)]
    viewports: Vec<String>,
    #[serde(default)]
    crops: bool,
    settle_ms: Option<u64>,
    max_scroll_steps: Option<u64>,
    timeout_ms: Option<u64>,
    storage_state: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListRunsArgs {
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ComponentsArgs {
    run_id: String,
    target: Option<String>,
    component_id: Option<String>,
    role: Option<String>,
    name: Option<String>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CropArgs {
    run_id: String,
    target: Option<String>,
    component_id: String,
}

/// Standard base64, which MCP uses for pictures.
pub(crate) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}
