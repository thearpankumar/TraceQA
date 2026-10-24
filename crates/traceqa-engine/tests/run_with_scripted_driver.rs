//! Runs the engine against a scripted driver, so the paths that are hard to
//! reach with a real browser (failures, missing capabilities, cancelling) are covered.

// Integration tests: a failed unwrap is the failure (clippy.toml covers only #[test] functions and test modules).
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use traceqa_engine::driver::{Driver, DriverError, Drivers, ErrorKind, Transport};
use traceqa_engine::{Execution, Run, UrlPolicy};
use traceqa_protocol::types::{
    BrowserTarget, ComponentsFile, Event, ExtractOptions, RunManifest, RunStartParams, RunStatus, StorageState,
    Viewport,
};

const COLLECTED: &str = include_str!("../../../spec/examples/CollectorResult.shop.json");

/// Answers each method the way a driver would, and remembers what it was asked.
struct Scripted {
    calls: Arc<Mutex<Vec<String>>>,
    full_page: bool,
    /// A method that fails, and the kind of error it fails with.
    fail: Option<(&'static str, &'static str)>,
    /// How far down the pretend page is scrolled.
    y: f64,
    /// Counts sessions open at the same moment, across every scripted driver of a test.
    sessions: Arc<Sessions>,
    /// Whether the driver says its browser allows one session at a time.
    exclusive: bool,
    /// How many more page loads time out, across every scripted driver of a test.
    slow_loads: Arc<AtomicUsize>,
    /// The viewport the pretend browser really gives, when it cannot give the one asked for.
    reaches: Option<(u64, u64)>,
    /// What the pretend page reports as its final address, and as the address of a frame it cannot read.
    final_url: String,
    frame_url: String,
    /// Every address the engine asked the driver to open.
    opened: Arc<Mutex<Vec<String>>>,
    /// The redirect chain the pretend page reports, as JSON, when it reports one.
    chain: Option<&'static str>,
    /// What a failing method says.
    fail_text: &'static str,
    /// The `requestsFailed` the pretend page reports with every page.eval, as JSON, when it reports one.
    requests_failed: Option<&'static str>,
}

#[derive(Default)]
struct Sessions {
    open: AtomicUsize,
    most_at_once: AtomicUsize,
}

/// The pretend page: 2400 CSS pixels tall, seen through a 400-pixel-high viewport.
const PAGE_HEIGHT: f64 = 2400.0;
const VIEW_HEIGHT: f64 = 400.0;

/// Takes one from `counter` unless it is already 0. Written out, because the standard method
/// for this is renamed in newer Rust and the old name is deprecated there.
fn take_one(counter: &AtomicUsize) -> bool {
    let mut left = counter.load(Ordering::SeqCst);
    while left > 0 {
        match counter.compare_exchange(left, left - 1, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return true,
            Err(now) => left = now,
        }
    }
    false
}

impl Transport for Scripted {
    fn call(&mut self, method: &str, params: Value, _timeout: Duration) -> Result<Value, DriverError> {
        // The collector and the quick listing of controls taken while scrolling are told apart,
        // so a test can count how often the page was really read.
        let collector = params["script"]
            .as_str()
            .is_some_and(|script| script.contains("traceqaCollect"));
        let label = if method == "page.eval" && !collector {
            "page.eval:controls"
        } else {
            method
        };
        self.calls.lock().unwrap().push(label.to_string());
        if method == "page.goto" {
            self.opened
                .lock()
                .unwrap()
                .push(params["url"].as_str().unwrap().to_string());
        }
        if method == "page.goto" && take_one(&self.slow_loads) {
            return Err(DriverError::new(ErrorKind::Timeout, "the page did not load in time"));
        }
        if let Some((_, kind)) = self.fail.filter(|(failing, _)| *failing == method) {
            assert!(kind != "panic", "scripted driver exploded");
            return Err(DriverError::new(ErrorKind::from(kind), self.fail_text));
        }
        Ok(match method {
            "driver.hello" => json!({
                "driverName": "scripted", "driverVersion": "0", "protocolVersion": 1, "browsers": ["firefox"],
                "exclusiveBrowsers": if self.exclusive { vec!["firefox"] } else { vec![] }
            }),
            "session.open" => {
                let open = self.sessions.open.fetch_add(1, Ordering::SeqCst) + 1;
                self.sessions.most_at_once.fetch_max(open, Ordering::SeqCst);
                // Long enough for another worker to open its session too, if nothing stops it.
                std::thread::sleep(Duration::from_millis(30));
                let mut opened = json!({
                    "sessionId": "s1",
                    "browser": { "name": "firefox", "version": "1.0" },
                    "capabilities": {
                        "clipScreenshot": false, "fullPageScreenshot": self.full_page, "evalAllFrames": true,
                        "crossOriginFrames": false, "parallelSessions": false, "setViewport": true,
                        "consoleEvents": false, "networkEvents": false
                    }
                });
                if let Some((width, height)) = self.reaches {
                    opened["viewport"] = json!({ "width": width, "height": height });
                }
                opened
            }
            "page.goto" => {
                let mut page = json!({ "finalUrl": self.final_url, "status": 200, "title": "Buggy Shop" });
                if let Some(chain) = self.chain {
                    page["redirectChain"] = serde_json::from_str(chain).unwrap();
                }
                page
            }
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
                let mut evaluated = json!({ "results": [
                    { "frame": { "id": "main", "url": "https://shop.test/", "crossOrigin": false }, "value": collected },
                    { "frame": { "id": "f1", "url": self.frame_url, "crossOrigin": true },
                      "error": "blocked" }
                ]});
                if let Some(failed) = self.requests_failed {
                    evaluated["requestsFailed"] = serde_json::from_str(failed).unwrap();
                }
                evaluated
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
            "session.close" => {
                self.sessions.open.fetch_sub(1, Ordering::SeqCst);
                json!({})
            }
            "driver.shutdown" => json!({}),
            other => panic!("the engine called {other}, which this test does not expect"),
        })
    }
}

struct Outcome {
    manifest: RunManifest,
    folder: PathBuf,
    events: Vec<Event>,
    calls: Vec<String>,
    /// Addresses the driver was asked to open.
    opened: Vec<String>,
}

fn run(full_page: bool, fail: Option<(&'static str, &'static str)>, cancelled: bool) -> Outcome {
    run_with(full_page, fail, cancelled, true)
}

/// One browser at one viewport, with or without pictures.
fn run_with(full_page: bool, fail: Option<(&'static str, &'static str)>, cancelled: bool, crops: bool) -> Outcome {
    run_reaching(full_page, fail, cancelled, crops, None)
}

/// As `run_with`, with a pretend browser that may not reach the viewport asked for.
fn run_reaching(
    full_page: bool,
    fail: Option<(&'static str, &'static str)>,
    cancelled: bool,
    crops: bool,
    reaches: Option<(u64, u64)>,
) -> Outcome {
    run_case(Case {
        full_page,
        fail,
        cancelled,
        crops,
        reaches,
        ..Case::default()
    })
}

/// Everything a single-target run can vary.
struct Case {
    full_page: bool,
    fail: Option<(&'static str, &'static str)>,
    cancelled: bool,
    crops: bool,
    reaches: Option<(u64, u64)>,
    url: &'static str,
    final_url: &'static str,
    frame_url: &'static str,
    storage_state: Option<StorageState>,
    chain: Option<&'static str>,
    policy: Option<UrlPolicy>,
    fail_text: &'static str,
    requests_failed: Option<&'static str>,
}

impl Default for Case {
    fn default() -> Self {
        Self {
            full_page: true,
            fail: None,
            cancelled: false,
            crops: true,
            reaches: None,
            url: "https://shop.test",
            final_url: "https://shop.test/",
            frame_url: "https://pay.example.com/widget",
            storage_state: None,
            chain: None,
            policy: None,
            fail_text: "connection refused",
            requests_failed: None,
        }
    }
}

// Callers build a `Case` with struct-update syntax at each call; by value reads best there.
#[allow(clippy::needless_pass_by_value)]
fn run_case(case: Case) -> Outcome {
    let out = std::env::temp_dir().join("traceqa-engine-tests");
    let request = RunStartParams {
        feature: "extract".into(),
        url: case.url.into(),
        targets: vec![BrowserTarget::named("firefox")],
        viewports: vec![Viewport {
            width: 1280,
            height: 400,
        }],
        out_dir: Some(out.to_string_lossy().into_owned()),
        parallel: None,
        options: Some(ExtractOptions {
            crops: Some(case.crops),
            include_hidden: None,
            max_scroll_steps: None,
            settle_ms: None,
            timeout_ms: None,
        }),
        storage_state: case.storage_state.clone(),
    };
    let run = Run::prepare(request).unwrap();
    let folder = run.folder().to_path_buf();

    let calls = Arc::new(Mutex::new(Vec::new()));
    let opened = Arc::new(Mutex::new(Vec::new()));
    let scripted = || {
        let calls = Arc::clone(&calls);
        let opened = Arc::clone(&opened);
        let start = move || {
            let transport = Scripted {
                y: 0.0,
                calls: Arc::clone(&calls),
                full_page: case.full_page,
                fail: case.fail,
                sessions: Arc::default(),
                exclusive: false,
                slow_loads: Arc::default(),
                reaches: case.reaches,
                final_url: case.final_url.into(),
                frame_url: case.frame_url.into(),
                opened: Arc::clone(&opened),
                chain: case.chain,
                fail_text: case.fail_text,
                requests_failed: case.requests_failed,
            };
            Driver::connect(Box::new(transport))
        };
        Drivers::starting_with(vec![Box::new(start)])
    };
    let cancel = AtomicBool::new(case.cancelled);
    let mut events = Vec::new();
    let manifest = run.execute(
        &Execution {
            policy: case.policy.as_ref(),
            drivers: &scripted,
            cancel: &cancel,
        },
        &mut |event| events.push(event.clone()),
    );
    let calls = calls.lock().unwrap().clone();
    let opened = opened.lock().unwrap().clone();
    Outcome {
        manifest,
        folder,
        events,
        calls,
        opened,
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
    let outcome = run(true, Some(("page.goto", ErrorKind::Timeout.as_str())), false);
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
    // Not even a driver is started.
    assert!(outcome.calls.is_empty());
    fs::remove_dir_all(outcome.folder).unwrap();
}

/// Runs three browsers at two viewports, `parallel` at a time, and reports the most sessions open at once.
fn run_six_targets(parallel: u64, exclusive: bool) -> (RunManifest, Vec<Event>, usize, PathBuf) {
    run_six_targets_with(parallel, exclusive, 0)
}

/// As `run_six_targets`, with the first `slow_loads` page loads timing out.
fn run_six_targets_with(
    parallel: u64,
    exclusive: bool,
    slow_loads: usize,
) -> (RunManifest, Vec<Event>, usize, PathBuf) {
    let request = RunStartParams {
        feature: "extract".into(),
        url: "https://shop.test".into(),
        targets: vec![
            BrowserTarget::named("firefox"),
            BrowserTarget::named("firefox"),
            BrowserTarget::named("firefox"),
        ],
        viewports: vec![
            Viewport {
                width: 1280,
                height: 400,
            },
            Viewport {
                width: 390,
                height: 400,
            },
        ],
        out_dir: Some(
            std::env::temp_dir()
                .join("traceqa-engine-tests")
                .to_string_lossy()
                .into_owned(),
        ),
        parallel: Some(parallel),
        options: None,
        storage_state: None,
    };
    let run = Run::prepare(request).unwrap();
    let folder = run.folder().to_path_buf();
    let sessions = Arc::new(Sessions::default());
    let slow_loads = Arc::new(AtomicUsize::new(slow_loads));
    let scripted = || {
        let (sessions, slow_loads) = (Arc::clone(&sessions), Arc::clone(&slow_loads));
        let start = move || {
            let transport = Scripted {
                y: 0.0,
                calls: Arc::default(),
                full_page: true,
                fail: None,
                sessions: Arc::clone(&sessions),
                exclusive,
                slow_loads: Arc::clone(&slow_loads),
                reaches: None,
                final_url: "https://shop.test/".into(),
                frame_url: "https://pay.example.com/widget".into(),
                opened: Arc::default(),
                chain: None,
                fail_text: "connection refused",
                requests_failed: None,
            };
            Driver::connect(Box::new(transport))
        };
        Drivers::starting_with(vec![Box::new(start)])
    };
    let cancel = AtomicBool::new(false);
    let mut events = Vec::new();
    let manifest = run.execute(
        &Execution {
            policy: None,
            drivers: &scripted,
            cancel: &cancel,
        },
        &mut |event| events.push(event.clone()),
    );
    (manifest, events, sessions.most_at_once.load(Ordering::SeqCst), folder)
}

#[test]
fn targets_run_side_by_side_and_are_reported_in_the_order_asked_for() {
    let (manifest, events, most_at_once, folder) = run_six_targets(3, false);
    assert_eq!(manifest.status, RunStatus::Completed);
    // Whatever order they finished in, the manifest lists them as they were requested.
    let dirs: Vec<&str> = manifest.targets.iter().map(|target| target.dir.as_str()).collect();
    assert_eq!(
        dirs,
        [
            "firefox-1280x400",
            "firefox-390x400",
            "firefox-1280x400-2",
            "firefox-390x400-2",
            "firefox-1280x400-3",
            "firefox-390x400-3"
        ]
    );
    assert!(manifest
        .targets
        .iter()
        .all(|target| target.status == RunStatus::Completed && target.timings.is_some()));
    assert!(
        (2..=3).contains(&most_at_once),
        "ran {most_at_once} at once, asked for 3"
    );

    // Events from all targets form one numbered sequence, and the file holds the same sequence.
    assert!(events.windows(2).all(|pair| pair[1].seq == pair[0].seq + 1));
    let logged = fs::read_to_string(folder.join("events.jsonl")).unwrap().lines().count();
    assert_eq!(logged, events.len());
    for target in &manifest.targets {
        assert!(folder.join(&target.dir).join("components.json").is_file());
    }
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn one_at_a_time_means_one_at_a_time() {
    let (manifest, _, most_at_once, folder) = run_six_targets(1, false);
    assert_eq!(manifest.status, RunStatus::Completed);
    assert_eq!(most_at_once, 1);
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn a_browser_that_allows_one_session_is_never_opened_twice_at_once() {
    // Three workers are free to run, but the driver says its browser is exclusive.
    let (manifest, _, most_at_once, folder) = run_six_targets(3, true);
    assert_eq!(manifest.status, RunStatus::Completed);
    assert_eq!(most_at_once, 1);
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn a_target_that_times_out_beside_others_is_tried_again_on_its_own() {
    let (manifest, events, _, folder) = run_six_targets_with(3, false, 2);
    // Two page loads timed out while three browsers ran. Each was retried alone and then worked.
    assert_eq!(manifest.status, RunStatus::Completed);
    assert!(manifest
        .targets
        .iter()
        .all(|target| target.status == RunStatus::Completed && target.error.is_none()));
    let kinds = |kind: &str| events.iter().filter(|event| event.type_ == kind).count();
    assert_eq!(kinds("target.retried"), 2);
    assert_eq!(kinds("error"), 2);
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn a_target_that_times_out_alone_is_not_tried_again() {
    // With one browser at a time there was nothing competing for the machine, so a retry would only repeat it.
    let (manifest, events, _, folder) = run_six_targets_with(1, false, 1);
    assert_eq!(manifest.status, RunStatus::Failed);
    assert_eq!(
        manifest
            .targets
            .iter()
            .filter(|target| target.status == RunStatus::Failed)
            .count(),
        1
    );
    assert!(events.iter().all(|event| event.type_ != "target.retried"));
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn without_pictures_the_page_is_scrolled_but_never_photographed() {
    let pictures = run_with(true, None, false, true);
    let data_only = run_with(true, None, false, false);
    assert_eq!(data_only.manifest.status, RunStatus::Completed);

    // No screenshot is taken and the page is read twice: once before the walk, to see later which
    // names changed, and once after scrolling to the end and back. On the way, only the quick
    // listing of controls runs, once before scrolling and after each scroll.
    let count = |outcome: &Outcome, method: &str| outcome.calls.iter().filter(|call| *call == method).count();
    assert_eq!(count(&data_only, "page.screenshot"), 0);
    assert_eq!(count(&data_only, "page.eval"), 2);
    assert!(count(&data_only, "page.eval:controls") >= 2);
    assert!(count(&data_only, "page.scroll") >= 2);
    assert!(count(&pictures, "page.eval") > 1 && count(&pictures, "page.screenshot") > 1);

    // The folder holds the data and nothing else.
    let target = data_only.folder.join("firefox-1280x400");
    let files: Vec<String> = fs::read_dir(&target)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(files, ["components.json"]);

    // The components are the same either way. Only the pictures differ.
    let ids = |outcome: &Outcome| {
        components(outcome)
            .components
            .iter()
            .map(|c| c.id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&data_only), ids(&pictures));
    assert!(components(&data_only)
        .components
        .iter()
        .all(|component| component.crop.is_none()));
    assert!(components(&pictures)
        .components
        .iter()
        .all(|component| component.crop.is_some()));
    fs::remove_dir_all(data_only.folder).unwrap();
    fs::remove_dir_all(pictures.folder).unwrap();
}

#[test]
fn a_viewport_the_browser_cannot_reach_is_recorded_as_it_really_was() {
    // Asked for 1280x400. The pretend browser, like Safari in a window macOS will not resize, gives 1280x600.
    let outcome = run_reaching(true, None, false, false, Some((1280, 600)));
    let target = &outcome.manifest.targets[0];
    assert_eq!(target.status, RunStatus::Completed);
    // The target keeps the name and viewport it was asked for, so runs stay comparable.
    assert_eq!((target.dir.as_str(), target.viewport.height), ("firefox-1280x400", 400));

    let file = components(&outcome);
    assert_eq!((file.viewport.width, file.viewport.height), (1280, 600));
    let warning = file
        .warnings
        .iter()
        .find(|warning| warning.code == "viewport_not_reached")
        .expect("a viewport_not_reached warning");
    assert!(warning.message.contains("1280x400") && warning.message.contains("1280x600"));
    assert_eq!(target.warning_count, file.warnings.len() as u64);
    assert!(outcome.events.iter().any(|event| event.type_ == "warning"
        && event.data.get("code").and_then(|code| code.as_str()) == Some("viewport_not_reached")));
    fs::remove_dir_all(outcome.folder).unwrap();
}

#[test]
fn a_viewport_that_is_reached_raises_no_warning() {
    let outcome = run_reaching(true, None, false, false, Some((1280, 400)));
    let file = components(&outcome);
    assert_eq!((file.viewport.width, file.viewport.height), (1280, 400));
    assert!(file
        .warnings
        .iter()
        .all(|warning| warning.code != "viewport_not_reached"));
    fs::remove_dir_all(outcome.folder).unwrap();
}

#[test]
fn failed_requests_reported_by_the_driver_become_one_warning() {
    let outcome = run_case(Case {
        crops: false,
        requests_failed: Some(r#"{ "count": 2, "urls": ["https://shop.test/app.js", "https://shop.test/api/items"] }"#),
        ..Case::default()
    });
    let target = &outcome.manifest.targets[0];
    assert_eq!(target.status, RunStatus::Completed);
    let file = components(&outcome);
    let failed: Vec<_> = file.warnings.iter().filter(|w| w.code == "requests_failed").collect();
    assert_eq!(failed.len(), 1, "{:?}", file.warnings);
    assert!(failed[0].message.starts_with("2 request(s)"));
    assert!(failed[0].message.contains("https://shop.test/app.js"));
    assert_eq!(target.warning_count, file.warnings.len() as u64);
    fs::remove_dir_all(outcome.folder).unwrap();
}

#[test]
fn a_driver_that_reports_no_failed_requests_raises_no_warning() {
    for reported in [None, Some(r#"{ "count": 0, "urls": [] }"#)] {
        let outcome = run_case(Case {
            crops: false,
            requests_failed: reported,
            ..Case::default()
        });
        assert!(components(&outcome)
            .warnings
            .iter()
            .all(|w| w.code != "requests_failed"));
        fs::remove_dir_all(outcome.folder).unwrap();
    }
}

/// Every file under `dir`, read as text where it is text.
fn all_text_files(dir: &std::path::Path) -> Vec<(PathBuf, String)> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(all_text_files(&path));
        } else if let Ok(text) = fs::read_to_string(&path) {
            found.push((path, text));
        }
    }
    found
}

#[test]
fn the_driver_receives_the_real_url_while_run_json_events_and_components_json_hold_the_redacted_one() {
    let outcome = run_case(Case {
        url: "https://shop.test/cb?token=abc&page=2",
        final_url: "https://shop.test/cb?token=abc&page=2#access_token=abc&x=1",
        crops: false,
        ..Case::default()
    });
    assert_eq!(outcome.manifest.status, RunStatus::Completed);
    // The browser still gets the real address.
    assert_eq!(outcome.opened, ["https://shop.test/cb?token=abc&page=2"]);

    assert_eq!(outcome.manifest.url, "https://shop.test/cb?token=REDACTED&page=2");
    let file = components(&outcome);
    assert_eq!(file.url, "https://shop.test/cb?token=REDACTED&page=2");
    assert_eq!(
        file.final_url,
        "https://shop.test/cb?token=REDACTED&page=2#access_token=REDACTED&x=1"
    );

    let files = all_text_files(&outcome.folder);
    assert!(files.iter().any(|(path, _)| path.ends_with("events.jsonl")));
    for (path, text) in &files {
        assert!(!text.contains("abc"), "{} holds the secret", path.display());
    }
    for event in &outcome.events {
        assert!(!serde_json::to_string(event).unwrap().contains("abc"), "{event:?}");
    }
}

#[test]
fn addresses_without_secrets_are_stored_exactly_as_given() {
    let outcome = run_case(Case {
        url: "https://Shop.test/a b?x=1&y=2#frag",
        final_url: "https://Shop.test/a%20b?x=1&y=2#frag",
        crops: false,
        ..Case::default()
    });
    assert_eq!(outcome.manifest.url, "https://Shop.test/a b?x=1&y=2#frag");
    let file = components(&outcome);
    assert_eq!(file.url, "https://Shop.test/a b?x=1&y=2#frag");
    assert_eq!(file.final_url, "https://Shop.test/a%20b?x=1&y=2#frag");
}

#[test]
fn a_final_url_and_frame_urls_are_redacted_too() {
    let outcome = run_case(Case {
        final_url: "https://shop.test/landing?api_key=SEKRET",
        frame_url: "https://pay.example.com/widget?sig=SEKRET",
        crops: false,
        ..Case::default()
    });
    let file = components(&outcome);
    assert_eq!(file.final_url, "https://shop.test/landing?api_key=REDACTED");
    let frame_warning = file
        .warnings
        .iter()
        .find(|warning| warning.code == "cross_origin_frame_skipped")
        .expect("the frame could not be read");
    assert!(
        frame_warning.message.contains("sig=REDACTED"),
        "{}",
        frame_warning.message
    );
    for (path, text) in all_text_files(&outcome.folder) {
        assert!(!text.contains("SEKRET"), "{} holds the secret", path.display());
    }
}

#[test]
fn a_sign_in_page_is_still_noticed_from_the_real_final_url() {
    let state: StorageState = serde_json::from_value(json!({ "cookies": [], "origins": [] })).unwrap();
    let outcome = run_case(Case {
        final_url: "https://shop.test/login?token=abc",
        storage_state: Some(state),
        crops: false,
        ..Case::default()
    });
    let file = components(&outcome);
    assert!(file.warnings.iter().any(|w| w.code == "session_not_accepted"));
}

#[test]
fn an_address_with_a_user_name_or_password_is_refused_before_a_run_folder_exists() {
    let out = std::env::temp_dir().join(format!("traceqa-engine-userinfo-{}", std::process::id()));
    let _ = fs::remove_dir_all(&out);
    for url in [
        "https://user:pw@example.com",
        "https://user@example.com/",
        "https://u:p\t@h/",
    ] {
        let request = RunStartParams {
            feature: "extract".into(),
            url: url.into(),
            targets: vec![BrowserTarget::named("firefox")],
            viewports: vec![Viewport {
                width: 1280,
                height: 400,
            }],
            out_dir: Some(out.to_string_lossy().into_owned()),
            parallel: None,
            options: None,
            storage_state: None,
        };
        let error = Run::prepare(request).err().expect("must be refused");
        assert!(error.to_string().contains("--storage-state"), "{error}");
        assert!(!error.to_string().contains("pw"), "{error}");
    }
    assert!(!out.exists(), "no run folder may be created");
}

fn empty_state() -> StorageState {
    serde_json::from_value(json!({ "cookies": [], "origins": [] })).unwrap()
}

fn signed_in_pictures_count(outcome: &Outcome) -> usize {
    components(outcome)
        .warnings
        .iter()
        .filter(|warning| warning.code == "signed_in_pictures")
        .count()
}

#[test]
fn a_signed_in_run_with_crops_warns_that_pictures_were_saved() {
    let outcome = run_case(Case {
        storage_state: Some(empty_state()),
        crops: true,
        ..Case::default()
    });
    assert_eq!(outcome.manifest.status, RunStatus::Completed);
    assert_eq!(signed_in_pictures_count(&outcome), 1);
    let warned = outcome
        .events
        .iter()
        .filter(|event| {
            event.type_ == "warning" && event.data.get("code").is_some_and(|code| code == "signed_in_pictures")
        })
        .count();
    assert_eq!(warned, 1, "one warning event");
    assert!(outcome.folder.join("firefox-1280x400").join("page.png").is_file());
    // The warning counts in the target's summary like any other.
    assert_eq!(
        outcome.manifest.targets[0].warning_count as usize,
        components(&outcome).warnings.len()
    );
}

#[test]
fn no_warning_without_crops_or_without_a_state() {
    let without_crops = run_case(Case {
        storage_state: Some(empty_state()),
        crops: false,
        ..Case::default()
    });
    assert_eq!(signed_in_pictures_count(&without_crops), 0);
    let without_state = run_case(Case {
        storage_state: None,
        crops: true,
        ..Case::default()
    });
    assert_eq!(signed_in_pictures_count(&without_state), 0);
    assert_eq!(signed_in_pictures_count(&run_case(Case::default())), 0);
}

fn nothing_resolves(_: &str) -> Vec<std::net::IpAddr> {
    Vec::new()
}

fn refused(case: Case) -> Outcome {
    let outcome = run_case(Case { crops: false, ..case });
    assert_eq!(outcome.manifest.status, RunStatus::Failed);
    let target = &outcome.manifest.targets[0];
    let error = target.error.as_deref().unwrap();
    assert!(error.contains("blocked_url"), "{error}");
    assert!(
        !outcome.folder.join("firefox-1280x400").join("components.json").exists(),
        "a refused page must leave no components.json"
    );
    // Nothing was read from the page, and its opening was not announced.
    assert!(!outcome
        .calls
        .iter()
        .any(|call| call.starts_with("page.eval") || call == "page.scroll"));
    assert!(!outcome.events.iter().any(|event| event.type_ == "page.opened"));
    for (path, text) in all_text_files(&outcome.folder) {
        assert!(!text.contains("SEKRET"), "{} holds the secret", path.display());
    }
    outcome
}

#[test]
fn a_redirect_chain_through_a_blocked_host_fails_the_target_and_writes_no_components() {
    let strict = || UrlPolicy::new(true, &[]).with_resolver(nothing_resolves);
    // A hop that is the metadata address, in the middle of a chain that ends somewhere fine.
    let outcome = refused(Case {
        chain: Some(
            r#"[{"url":"https://shop.test/"},{"url":"http://169.254.169.254/latest?token=SEKRET"},{"url":"https://shop.test/"}]"#,
        ),
        policy: Some(strict()),
        ..Case::default()
    });
    let error = outcome.manifest.targets[0].error.as_deref().unwrap();
    assert!(
        error.contains("169.254.169.254") && error.contains("already been made"),
        "{error}"
    );

    // The browser says it connected to a private address for a public name.
    refused(Case {
        chain: Some(r#"[{"url":"https://shop.test/","remoteAddress":"10.0.0.7"},{"url":"https://shop.test/"}]"#),
        policy: Some(strict()),
        ..Case::default()
    });
    // ... and to the metadata address, which the default rules refuse too.
    refused(Case {
        chain: Some(r#"[{"url":"https://shop.test/","remoteAddress":"::ffff:169.254.169.254"}]"#),
        policy: Some(UrlPolicy::default().with_resolver(nothing_resolves)),
        ..Case::default()
    });
    // A driver that gives no chain is judged by where the page ended.
    refused(Case {
        chain: None,
        final_url: "http://metadata.google.internal/computeMetadata?token=SEKRET",
        policy: Some(UrlPolicy::default().with_resolver(nothing_resolves)),
        ..Case::default()
    });
    refused(Case {
        chain: None,
        final_url: "http://192.168.1.1/",
        policy: Some(strict()),
        ..Case::default()
    });
}

#[test]
fn a_clean_chain_or_no_policy_leaves_the_run_alone() {
    let clean = r#"[{"url":"https://shop.test/","remoteAddress":"93.184.216.34"},{"url":"https://shop.test/home"}]"#;
    let with_policy = run_case(Case {
        chain: Some(clean),
        policy: Some(UrlPolicy::new(true, &[]).with_resolver(nothing_resolves)),
        crops: false,
        ..Case::default()
    });
    assert_eq!(with_policy.manifest.status, RunStatus::Completed);
    // Without a policy even a metadata hop changes nothing: the CLI and `serve` are unchanged.
    let without = run_case(Case {
        chain: Some(r#"[{"url":"http://169.254.169.254/"}]"#),
        crops: false,
        ..Case::default()
    });
    assert_eq!(without.manifest.status, RunStatus::Completed);
}

#[test]
fn a_failing_goto_with_a_token_in_its_error_leaves_no_token_in_the_run_folder() {
    let outcome = run_case(Case {
        fail: Some(("page.goto", "navigation_failed")),
        fail_text: "Page.goto: net::ERR_CONNECTION_REFUSED at https://shop.test/cb?token=SEKRET&page=2",
        crops: false,
        ..Case::default()
    });
    assert_eq!(outcome.manifest.status, RunStatus::Failed);
    let error = outcome.manifest.targets[0].error.as_deref().unwrap();
    assert!(error.contains("https://shop.test/cb?token=REDACTED&page=2"), "{error}");
    let files = all_text_files(&outcome.folder);
    assert!(files.iter().any(|(path, _)| path.ends_with("events.jsonl")));
    for (path, text) in &files {
        assert!(!text.contains("SEKRET"), "{} holds the secret", path.display());
    }
    for event in &outcome.events {
        assert!(!serde_json::to_string(event).unwrap().contains("SEKRET"), "{event:?}");
    }
}

#[test]
fn a_panicking_driver_ends_in_a_failed_run_folder() {
    let outcome = run(true, Some(("page.goto", "panic")), false);
    assert_eq!(outcome.manifest.status, RunStatus::Failed);
    let target = &outcome.manifest.targets[0];
    assert_eq!(target.status, RunStatus::Failed);
    let error = target.error.as_deref().unwrap();
    assert!(error.starts_with("internal error: "), "{error}");
    assert!(error.contains("scripted driver exploded"), "{error}");

    // run.json says failed, not running, and is the manifest that was returned.
    let on_disk: RunManifest =
        serde_json::from_str(&fs::read_to_string(outcome.folder.join("run.json")).unwrap()).unwrap();
    assert_eq!(on_disk, outcome.manifest);
    assert!(on_disk.finished_at.is_some());

    // events.jsonl has the failure and ends with run.finished, and matches what was sent live.
    let logged: Vec<Event> = fs::read_to_string(outcome.folder.join("events.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(logged, outcome.events);
    assert_eq!(logged.last().unwrap().type_, "run.finished");
    assert!(logged.iter().any(|event| event.type_ == "error"));
}
