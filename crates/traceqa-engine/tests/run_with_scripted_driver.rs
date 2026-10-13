//! Runs the engine against a scripted driver, so the paths that are hard to
//! reach with a real browser (failures, missing capabilities, cancelling) are covered.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use traceqa_engine::driver::{Driver, DriverError, Drivers, Transport};
use traceqa_engine::Run;
use traceqa_protocol::types::{BrowserTarget, ComponentsFile, Event, RunManifest, RunStartParams, RunStatus, Viewport};

const COLLECTED: &str = include_str!("../../../spec/examples/CollectorResult.shop.json");

/// Answers each method the way a driver would, and remembers what it was asked.
struct Scripted {
    calls: Arc<Mutex<Vec<String>>>,
    full_page: bool,
    /// A method that fails, and the kind of error it fails with.
    fail: Option<(&'static str, &'static str)>,
    /// How far down the pretend page is scrolled.
    y: f64,
}

/// The pretend page: 2400 CSS pixels tall, seen through a 400-pixel-high viewport.
const PAGE_HEIGHT: f64 = 2400.0;
const VIEW_HEIGHT: f64 = 400.0;

impl Transport for Scripted {
    fn call(&mut self, method: &str, params: Value, _timeout: Duration) -> Result<Value, DriverError> {
        self.calls.lock().unwrap().push(method.to_string());
        if let Some((_, kind)) = self.fail.filter(|(failing, _)| *failing == method) {
            return Err(DriverError {
                kind: kind.to_string(),
                message: "connection refused".into(),
            });
        }
        Ok(match method {
            "driver.hello" => json!({
                "driverName": "scripted", "driverVersion": "0", "protocolVersion": 1, "browsers": ["firefox"]
            }),
            "session.open" => json!({
                "sessionId": "s1",
                "browser": { "name": "firefox", "version": "1.0" },
                "capabilities": {
                    "clipScreenshot": false, "fullPageScreenshot": self.full_page, "evalAllFrames": true,
                    "crossOriginFrames": false, "parallelSessions": false, "setViewport": true,
                    "consoleEvents": false, "networkEvents": false
                }
            }),
            "page.goto" => json!({ "finalUrl": "https://shop.test/", "status": 200, "title": "Buggy Shop" }),
            "page.scroll" => {
                self.y = match params["byY"].as_f64() {
                    Some(by) => (self.y + by).min(PAGE_HEIGHT - VIEW_HEIGHT),
                    None => 0.0,
                };
                json!({ "y": self.y, "height": PAGE_HEIGHT, "atEnd": self.y >= PAGE_HEIGHT - VIEW_HEIGHT })
            }
            "page.eval" => {
                let mut collected: Value = serde_json::from_str(COLLECTED).unwrap();
                collected["scroll"] = json!({ "y": self.y, "height": PAGE_HEIGHT, "viewportHeight": VIEW_HEIGHT });
                json!({ "results": [
                    { "frame": { "id": "main", "url": "https://shop.test/", "crossOrigin": false }, "value": collected },
                    { "frame": { "id": "f1", "url": "https://pay.example.com/widget", "crossOrigin": true },
                      "error": "blocked" }
                ]})
            }
            "page.screenshot" => {
                let whole_page = params["fullPage"] == json!(true);
                assert!(
                    self.full_page || !whole_page,
                    "asked for a full-page screenshot the driver said it cannot take"
                );
                let (top, height) = if whole_page {
                    (0.0, PAGE_HEIGHT)
                } else {
                    (self.y, VIEW_HEIGHT)
                };
                // Every row is painted with its own page position, so a test can tell where a crop came from.
                let image = image::RgbImage::from_fn(1280, height as u32, |_, row| {
                    let page_y = top as u32 + row;
                    image::Rgb([(page_y / 256) as u8, (page_y % 256) as u8, 0])
                });
                image.save(params["path"].as_str().unwrap()).unwrap();
                json!({ "path": params["path"], "width": 1280, "height": height as u32 })
            }
            "session.close" | "driver.shutdown" => json!({}),
            other => panic!("the engine called {other}, which this test does not expect"),
        })
    }
}

struct Outcome {
    manifest: RunManifest,
    folder: PathBuf,
    events: Vec<Event>,
    calls: Vec<String>,
}

fn run(full_page: bool, fail: Option<(&'static str, &'static str)>, cancelled: bool) -> Outcome {
    let out = std::env::temp_dir().join("traceqa-engine-tests");
    let request = RunStartParams {
        feature: "extract".into(),
        url: "https://shop.test".into(),
        targets: vec![BrowserTarget::named("firefox")],
        viewports: vec![Viewport {
            width: 1280,
            height: 400,
        }],
        out_dir: Some(out.to_string_lossy().into_owned()),
        options: None,
    };
    let run = Run::prepare(request).unwrap();
    let folder = run.folder().to_path_buf();

    let calls = Arc::new(Mutex::new(Vec::new()));
    let transport = Scripted {
        y: 0.0,
        calls: Arc::clone(&calls),
        full_page,
        fail,
    };
    let mut drivers = Drivers::connected(vec![Driver::connect(Box::new(transport)).unwrap()]);
    let mut events = Vec::new();
    let manifest = run.execute(
        &mut drivers,
        &mut |event| events.push(event.clone()),
        &AtomicBool::new(cancelled),
    );
    let calls = calls.lock().unwrap().clone();
    Outcome {
        manifest,
        folder,
        events,
        calls,
    }
}

fn components(outcome: &Outcome) -> ComponentsFile {
    let path = outcome.folder.join("firefox-1280x400").join("components.json");
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn a_run_writes_the_whole_run_folder() {
    let outcome = run(true, None, false);
    assert_eq!(outcome.manifest.status, RunStatus::Completed);

    // run.json on disk is the manifest that was returned.
    let on_disk: RunManifest =
        serde_json::from_str(&fs::read_to_string(outcome.folder.join("run.json")).unwrap()).unwrap();
    assert_eq!(on_disk, outcome.manifest);
    let target = &outcome.manifest.targets[0];
    assert_eq!((target.component_count, target.instance_count), (2, 2));

    // events.jsonl holds exactly the events that were sent live, in order.
    let logged: Vec<Event> = fs::read_to_string(outcome.folder.join("events.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(logged, outcome.events);
    let kinds: Vec<&str> = logged.iter().map(|event| event.type_.as_str()).collect();
    assert_eq!(kinds.first(), Some(&"run.started"));
    assert_eq!(kinds.last(), Some(&"run.finished"));
    assert!(logged.windows(2).all(|pair| pair[1].seq == pair[0].seq + 1));

    let file = components(&outcome);
    for component in &file.components {
        let crop = component.crop.as_ref().expect("both components are visible");
        assert!(outcome.folder.join("firefox-1280x400").join(crop).is_file());
    }
    // The frame that could not be read is a warning, next to the collector's own.
    let codes: Vec<&str> = file.warnings.iter().map(|warning| warning.code.as_str()).collect();
    assert_eq!(codes, ["unnamed_interactive_element", "cross_origin_frame_skipped"]);
    assert_eq!(target.warning_count, 2);
    fs::remove_dir_all(outcome.folder).unwrap();
}

#[test]
fn a_failed_page_load_fails_the_target_and_still_closes_the_session() {
    let outcome = run(true, Some(("page.goto", "navigation_failed")), false);
    assert_eq!(outcome.manifest.status, RunStatus::Failed);
    let target = &outcome.manifest.targets[0];
    assert_eq!(target.status, RunStatus::Failed);
    assert!(target.error.as_ref().unwrap().contains("connection refused"));
    assert_eq!(
        outcome.calls,
        ["driver.hello", "session.open", "page.goto", "session.close"]
    );
    assert!(outcome.events.iter().any(|event| event.type_ == "error"));
    assert!(!outcome.folder.join("firefox-1280x400").join("components.json").exists());
    fs::remove_dir_all(outcome.folder).unwrap();
}

#[test]
fn a_driver_that_stops_answering_is_not_asked_for_anything_more() {
    let outcome = run(true, Some(("page.goto", "timeout")), false);
    assert_eq!(outcome.manifest.targets[0].status, RunStatus::Failed);
    // No session.close: the driver's state is unknown, so the engine leaves it and replaces it.
    assert_eq!(outcome.calls, ["driver.hello", "session.open", "page.goto"]);
    fs::remove_dir_all(outcome.folder).unwrap();
}

#[test]
fn a_screenshot_that_fails_costs_the_pictures_not_the_run() {
    let outcome = run(true, Some(("page.screenshot", "internal")), false);
    assert_eq!(outcome.manifest.status, RunStatus::Completed);
    let file = components(&outcome);
    assert_eq!(file.components.len(), 2);
    assert!(file.components.iter().all(|component| component.crop.is_none()));
    // Said once, however many screenshots failed.
    let failures = file
        .warnings
        .iter()
        .filter(|warning| warning.code == "screenshot_failed")
        .count();
    assert_eq!(failures, 1);
    fs::remove_dir_all(outcome.folder).unwrap();
}

/// The page y coordinate painted into the top row of a crop by the scripted driver.
fn page_y_of(crop: &std::path::Path) -> u32 {
    let pixel = *image::open(crop).unwrap().to_rgb8().get_pixel(0, 0);
    u32::from(pixel[0]) * 256 + u32::from(pixel[1])
}

#[test]
fn each_crop_comes_from_the_screen_that_showed_the_component() {
    for full_page in [true, false] {
        let outcome = run(full_page, None, false);
        assert_eq!(outcome.manifest.status, RunStatus::Completed);
        let target = outcome.folder.join("firefox-1280x400");
        let file = components(&outcome);

        // The promo is at y=120.5 on the first screen. The button is at y=480, a screen further down.
        // Each crop starts 4 pixels above its component, wherever the page was scrolled to when it was taken.
        let crop = |role: &str| {
            target.join(
                file.components
                    .iter()
                    .find(|c| c.role == role)
                    .unwrap()
                    .crop
                    .as_ref()
                    .unwrap(),
            )
        };
        assert_eq!(page_y_of(&crop("generic")), 116);
        assert_eq!(page_y_of(&crop("button")), 476);
        assert!(outcome.calls.iter().filter(|call| *call == "page.scroll").count() >= 2);

        // page.png is the whole page when the browser can capture it, else the first screen.
        let overview_height = image::image_dimensions(target.join("page.png")).unwrap().1;
        assert_eq!(overview_height, if full_page { 2400 } else { 400 });
        // The per-screen images are working files and are not left in the run folder.
        assert!(!target.join("screens").exists());
        fs::remove_dir_all(outcome.folder).unwrap();
    }
}

#[test]
fn a_cancelled_run_touches_no_browser() {
    let outcome = run(true, None, true);
    assert_eq!(outcome.manifest.status, RunStatus::Cancelled);
    assert_eq!(outcome.manifest.targets[0].status, RunStatus::Cancelled);
    assert_eq!(outcome.calls, ["driver.hello"]);
    fs::remove_dir_all(outcome.folder).unwrap();
}
