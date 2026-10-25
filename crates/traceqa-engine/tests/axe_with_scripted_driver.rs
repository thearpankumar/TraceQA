//! `--axe` against a scripted driver: what the engine sends, where it writes axe.json, and that a
//! target where axe cannot run still completes with a warning.

// Integration tests: a failed unwrap is the failure (clippy.toml covers only #[test] functions and test modules).
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

use serde_json::{json, Value};
use traceqa_engine::driver::{Driver, DriverError, Drivers, ErrorKind, Transport};
use traceqa_engine::{axe, Execution, Run};
use traceqa_protocol::types::{
    AxeResults, BrowserTarget, ComponentsFile, ExtractOptions, RunManifest, RunStartParams, RunStatus, Viewport,
};

const COLLECTED: &str = include_str!("../../../spec/examples/CollectorResult.shop.json");

/// A stand-in for axe.min.js: the engine only reads the file and sends it along.
const STUB_AXE: &str = "/* stub */ window.axe = { version: '0.0.0-stub', run: async () => ({}) };";

/// Points TRACEQA_AXE_FILE at the stub for every test of this file. They all use the same file.
fn stub_axe_file() -> PathBuf {
    static ONCE: Once = Once::new();
    let path = std::env::temp_dir().join("traceqa-axe-stub-test.js");
    ONCE.call_once(|| {
        fs::write(&path, STUB_AXE).unwrap();
        std::env::set_var(axe::FILE_ENV, &path);
    });
    path
}

#[derive(Clone, Copy)]
enum AxeAnswer {
    /// The shop button itself fails contrast; an image outside every component fails image-alt.
    Violations,
    /// The frame says no.
    Error,
}

struct Scripted {
    calls: Arc<Mutex<Vec<Value>>>,
    eval_async: bool,
    answer: AxeAnswer,
}

impl Transport for Scripted {
    fn call(&mut self, method: &str, params: Value, _timeout: Duration) -> Result<Value, DriverError> {
        if method == "page.eval" {
            self.calls.lock().unwrap().push(params.clone());
        }
        let is_axe = params["awaitPromise"] == json!(true);
        Ok(match method {
            "driver.hello" => json!({
                "driverName": "scripted", "driverVersion": "0", "protocolVersion": 1,
                "browsers": ["firefox"], "exclusiveBrowsers": []
            }),
            "session.open" => {
                let mut capabilities = json!({
                    "clipScreenshot": false, "fullPageScreenshot": false, "evalAllFrames": true,
                    "crossOriginFrames": false, "parallelSessions": false, "setViewport": true,
                    "consoleEvents": false, "networkEvents": false
                });
                if self.eval_async {
                    capabilities["evalAsync"] = json!(true);
                }
                json!({ "sessionId": "s1", "browser": { "name": "firefox", "version": "1.0" }, "capabilities": capabilities })
            }
            "page.goto" => json!({ "finalUrl": "https://shop.test/", "status": 200, "title": "Buggy Shop" }),
            "page.scroll" => json!({ "y": 0, "height": 400, "atEnd": true }),
            "page.eval" if is_axe => match self.answer {
                AxeAnswer::Error => {
                    return Err(DriverError::new(
                        ErrorKind::EvalFailed,
                        "axe-core did not load in this frame",
                    ))
                }
                AxeAnswer::Violations => json!({ "results": [{
                    "frame": { "id": "main", "url": "https://shop.test/", "crossOrigin": false },
                    "value": {
                        "version": "4.14.0", "incomplete": 2,
                        "violations": [
                            { "id": "color-contrast", "impact": "serious", "tags": ["wcag2aa", "wcag143"],
                              "help": "Elements must have sufficient colour contrast", "helpUrl": "https://example.test/cc",
                              "total": 1, "nodes": [{ "target": "#grid > li:nth-of-type(1) > button",
                                "box": { "x": 212, "y": 480, "w": 120, "h": 36 }, "tag": "button" }] },
                            { "id": "image-alt", "impact": "critical", "tags": ["wcag111"],
                              "help": "Images must have alternative text", "helpUrl": "https://example.test/ia",
                              "total": 1, "nodes": [{ "target": "img.hero",
                                "box": { "x": 900, "y": 1900, "w": 50, "h": 50 }, "tag": "img" }] }
                        ]
                    }
                }]}),
            },
            "page.eval" => {
                let mut collected: Value = serde_json::from_str(COLLECTED).unwrap();
                collected["scroll"] = json!({ "y": 0, "height": 400, "viewportHeight": 400 });
                json!({ "results": [{
                    "frame": { "id": "main", "url": "https://shop.test/", "crossOrigin": false },
                    "value": collected
                }]})
            }
            "session.close" | "driver.shutdown" => json!({}),
            other => panic!("the engine called {other}, which this test does not expect"),
        })
    }
}

struct Outcome {
    manifest: RunManifest,
    target_dir: PathBuf,
    evals: Vec<Value>,
}

fn run(axe_option: Option<bool>, eval_async: bool, answer: AxeAnswer) -> Outcome {
    stub_axe_file();
    let out = std::env::temp_dir().join("traceqa-engine-axe-tests");
    let request = RunStartParams {
        feature: "extract".into(),
        url: "https://shop.test".into(),
        targets: vec![BrowserTarget::named("firefox")],
        viewports: vec![Viewport {
            width: 1280,
            height: 400,
        }],
        out_dir: Some(out.to_string_lossy().into_owned()),
        parallel: None,
        options: Some(ExtractOptions {
            crops: Some(false),
            include_hidden: None,
            max_scroll_steps: None,
            settle_ms: None,
            timeout_ms: None,
            axe: axe_option,
        }),
        storage_state: None,
    };
    let run = Run::prepare(request).unwrap();
    let folder = run.folder().to_path_buf();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let scripted = || {
        let calls = Arc::clone(&calls);
        let start = move || {
            Driver::connect(Box::new(Scripted {
                calls: Arc::clone(&calls),
                eval_async,
                answer,
            }))
        };
        Drivers::starting_with(vec![Box::new(start)])
    };
    let cancel = AtomicBool::new(false);
    let manifest = run.execute(
        &Execution {
            policy: None,
            drivers: &scripted,
            cancel: &cancel,
        },
        &mut |_| {},
    );
    let evals = calls.lock().unwrap().clone();
    Outcome {
        manifest,
        target_dir: folder.join("firefox-1280x400"),
        evals,
    }
}

fn components(outcome: &Outcome) -> ComponentsFile {
    serde_json::from_str(&fs::read_to_string(outcome.target_dir.join("components.json")).unwrap()).unwrap()
}

#[test]
fn violations_are_written_next_to_the_components_and_placed_on_them() {
    let outcome = run(Some(true), true, AxeAnswer::Violations);
    assert_eq!(outcome.manifest.status, RunStatus::Completed);
    let file = components(&outcome);
    assert!(
        file.warnings.iter().all(|w| w.code != "axe_failed"),
        "{:?}",
        file.warnings
    );

    let results: AxeResults =
        serde_json::from_str(&fs::read_to_string(outcome.target_dir.join("axe.json")).unwrap()).unwrap();
    assert_eq!(results.axe_version, "4.14.0");
    assert_eq!(results.incomplete, 2);
    let contrast = results.violations.iter().find(|v| v.rule == "color-contrast").unwrap();
    let button = file
        .components
        .iter()
        .find(|c| c.instances.iter().any(|i| i.box_.x == 212.0 && i.box_.y == 480.0))
        .unwrap();
    assert_eq!(contrast.nodes[0].component_id.as_deref(), Some(button.id.as_str()));
    assert_eq!(
        contrast.nodes[0].via_ancestor, None,
        "same box and tag: the component itself"
    );
    let image = results.violations.iter().find(|v| v.rule == "image-alt").unwrap();
    assert_eq!(
        image.nodes[0].component_id, None,
        "an image no component contains is about the page"
    );
    // The page's html is never kept.
    let text = fs::read_to_string(outcome.target_dir.join("axe.json")).unwrap();
    assert!(!text.contains("html\""), "{text}");
}

#[test]
fn axe_is_sent_through_the_driver_as_a_function_that_waits() {
    let outcome = run(Some(true), true, AxeAnswer::Violations);
    let sent: Vec<&Value> = outcome
        .evals
        .iter()
        .filter(|p| p["awaitPromise"] == json!(true))
        .collect();
    assert_eq!(sent.len(), 1, "one axe call per target");
    let script = sent[0]["script"].as_str().unwrap();
    assert!(script.starts_with("async (tags) => {"));
    assert!(
        script.contains(STUB_AXE),
        "axe's source is inside the function, unmodified"
    );
    assert!(script.contains("iframes: false"));
    assert_eq!(sent[0]["allFrames"], json!(true));
    assert_eq!(sent[0]["arg"], json!(axe::TAGS));
    assert!(
        !script.contains("eval("),
        "the page's own eval is never used, so a Content-Security-Policy cannot block it"
    );
}

#[test]
fn a_default_run_does_not_touch_axe() {
    for option in [None, Some(false)] {
        let outcome = run(option, true, AxeAnswer::Violations);
        assert_eq!(outcome.manifest.status, RunStatus::Completed);
        assert!(!outcome.target_dir.join("axe.json").exists());
        assert!(
            outcome.evals.iter().all(|p| p.get("awaitPromise").is_none()),
            "no call waits for a promise"
        );
        assert!(!components(&outcome).warnings.iter().any(|w| w.code == "axe_failed"));
    }
}

#[test]
fn a_driver_that_cannot_wait_gets_a_warning_and_the_run_still_completes() {
    let outcome = run(Some(true), false, AxeAnswer::Violations);
    assert_eq!(outcome.manifest.status, RunStatus::Completed);
    let warning = components(&outcome)
        .warnings
        .into_iter()
        .find(|w| w.code == "axe_failed")
        .unwrap();
    assert!(warning.message.contains("evalAsync"), "{}", warning.message);
    assert!(!outcome.target_dir.join("axe.json").exists());
    assert!(
        outcome.evals.iter().all(|p| p["awaitPromise"] != json!(true)),
        "nothing was sent"
    );
}

#[test]
fn an_axe_that_fails_in_the_page_is_a_warning_not_a_failed_run() {
    let outcome = run(Some(true), true, AxeAnswer::Error);
    assert_eq!(outcome.manifest.status, RunStatus::Completed);
    assert_eq!(outcome.manifest.targets[0].status, RunStatus::Completed);
    let file = components(&outcome);
    let warning = file.warnings.iter().find(|w| w.code == "axe_failed").unwrap();
    assert!(warning.message.contains("did not load"), "{}", warning.message);
    assert!(!file.components.is_empty(), "the components are still there");
    assert!(!outcome.target_dir.join("axe.json").exists());
}
