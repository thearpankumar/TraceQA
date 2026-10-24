//! A run that is cancelled, or aborted, still ends with a folder that tells the truth: in-flight
//! targets are `cancelled` and not `failed`, and an abort closes `run.json` and `events.jsonl`
//! within moments even when a driver never answers.

// Integration tests: a failed unwrap is the failure (clippy.toml covers only #[test] functions and test modules).
#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use traceqa_engine::driver::{Driver, DriverError, Drivers, ErrorKind, Transport};
use traceqa_engine::{Abort, Execution, Run};
use traceqa_protocol::types::{BrowserTarget, Event, RunManifest, RunStartParams, RunStatus, Viewport};

/// What `page.goto` does.
#[derive(Clone)]
enum Goto {
    /// Fails with a transport error, as a driver that died under the target does. `cancel` is set
    /// after `cancel_after`, or at once when there is no delay.
    Dies {
        cancel: Arc<AtomicBool>,
        cancel_after: Option<Duration>,
    },
    /// Does not answer until the time has passed.
    Hangs(Duration),
}

struct Stub {
    goto: Goto,
}

impl Transport for Stub {
    fn call(&mut self, method: &str, _params: Value, _timeout: Duration) -> Result<Value, DriverError> {
        Ok(match method {
            "driver.hello" => json!({
                "driverName": "stub", "driverVersion": "0", "protocolVersion": 1, "browsers": ["firefox"]
            }),
            "session.open" => json!({
                "sessionId": "s1",
                "browser": { "name": "firefox", "version": "1.0" },
                "capabilities": {
                    "clipScreenshot": false, "fullPageScreenshot": true, "evalAllFrames": true,
                    "crossOriginFrames": false, "parallelSessions": false, "setViewport": true,
                    "consoleEvents": false, "networkEvents": false
                }
            }),
            "page.goto" => match &self.goto {
                Goto::Dies { cancel, cancel_after } => {
                    match cancel_after {
                        None => cancel.store(true, Ordering::SeqCst),
                        Some(delay) => {
                            let (cancel, delay) = (Arc::clone(cancel), *delay);
                            std::thread::spawn(move || {
                                std::thread::sleep(delay);
                                cancel.store(true, Ordering::SeqCst);
                            });
                        }
                    }
                    return Err(DriverError::new(
                        ErrorKind::Transport,
                        "the driver exited while handling page.goto",
                    ));
                }
                Goto::Hangs(how_long) => {
                    std::thread::sleep(*how_long);
                    return Err(DriverError::new(ErrorKind::Transport, "the driver went away"));
                }
            },
            "session.close" | "driver.shutdown" => json!({}),
            other => panic!("the engine called {other}, which this test does not expect"),
        })
    }
}

fn request(targets: usize) -> RunStartParams {
    RunStartParams {
        feature: "extract".into(),
        url: "https://shop.test".into(),
        targets: vec![BrowserTarget::named("firefox"); targets],
        viewports: vec![Viewport {
            width: 1280,
            height: 400,
        }],
        out_dir: Some(
            std::env::temp_dir()
                .join("traceqa-engine-abort-tests")
                .to_string_lossy()
                .into_owned(),
        ),
        parallel: Some(1),
        options: None,
        storage_state: None,
    }
}

fn drivers_for(goto: Goto) -> impl Fn() -> Drivers + Sync {
    move || {
        let goto = goto.clone();
        Drivers::starting_with(vec![Box::new(move || {
            Driver::connect(Box::new(Stub { goto: goto.clone() }))
        })])
    }
}

fn read_manifest(folder: &Path) -> RunManifest {
    serde_json::from_str(&fs::read_to_string(folder.join("run.json")).unwrap()).unwrap()
}

fn last_event(folder: &Path) -> Event {
    let text = fs::read_to_string(folder.join("events.jsonl")).unwrap();
    serde_json::from_str(text.lines().last().unwrap()).unwrap()
}

/// Runs one target whose driver dies, with the cancel flag set at once or after a delay.
fn run_dying(cancel_after: Option<Duration>) -> (RunManifest, Vec<Event>, PathBuf) {
    let run = Run::prepare(request(1)).unwrap();
    let folder = run.folder().to_path_buf();
    let cancel = Arc::new(AtomicBool::new(false));
    let drivers = drivers_for(Goto::Dies {
        cancel: Arc::clone(&cancel),
        cancel_after,
    });
    let mut events = Vec::new();
    let manifest = run.execute(
        &Execution {
            drivers: &drivers,
            cancel: &cancel,
            policy: None,
        },
        &mut |event| events.push(event.clone()),
    );
    (manifest, events, folder)
}

#[test]
fn cancelled_flag_turns_a_driver_error_into_a_cancelled_target() {
    for delay in [None, Some(Duration::from_millis(100))] {
        let (manifest, events, folder) = run_dying(delay);
        let target = &manifest.targets[0];
        assert_eq!(target.status, RunStatus::Cancelled, "{delay:?}: {target:?}");
        assert_eq!(manifest.status, RunStatus::Cancelled);
        assert!(
            target.error.is_none(),
            "a cancelled target needs no error: {:?}",
            target.error
        );
        assert!(
            events
                .iter()
                .any(|event| event.type_ == "target.cancelled" && event.message.contains("driver exited")),
            "the driver's words stay in an event"
        );
        assert!(!events.iter().any(|event| event.type_ == "error"));
        assert_eq!(read_manifest(&folder).status, RunStatus::Cancelled);
        fs::remove_dir_all(folder).unwrap();
    }
}

#[test]
fn a_driver_that_dies_without_any_cancel_is_a_failed_target() {
    let run = Run::prepare(request(1)).unwrap();
    let folder = run.folder().to_path_buf();
    let never = AtomicBool::new(false);
    // The stub would set its own flag in an hour; the run's flag is never set.
    let drivers = drivers_for(Goto::Dies {
        cancel: Arc::new(AtomicBool::new(false)),
        cancel_after: Some(Duration::from_secs(3600)),
    });
    let manifest = run.execute(
        &Execution {
            drivers: &drivers,
            cancel: &never,
            policy: None,
        },
        &mut |_| {},
    );
    assert_eq!(manifest.targets[0].status, RunStatus::Failed);
    assert_eq!(manifest.status, RunStatus::Failed);
    assert!(manifest.targets[0].error.as_deref().unwrap().contains("driver exited"));
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn an_abort_closes_the_run_folder_while_a_target_is_stuck() {
    let run = Run::prepare(request(2)).unwrap();
    let folder = run.folder().to_path_buf();
    let cancel = AtomicBool::new(false);
    let abort = Abort::new();
    let drivers = drivers_for(Goto::Hangs(Duration::from_secs(3)));
    let mut events = Vec::new();
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            run.execute_abortable(
                &Execution {
                    drivers: &drivers,
                    cancel: &cancel,
                    policy: None,
                },
                &abort,
                &mut |event| events.push(event.clone()),
            )
        });
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(read_manifest(&folder).status, RunStatus::Running);
        let asked = Instant::now();
        abort.request();
        assert!(
            abort.wait_closed(Duration::from_secs(2)),
            "the folder was not closed in time"
        );
        let took = asked.elapsed();
        assert!(took < Duration::from_millis(1500), "took {took:?}");

        // The run is still stuck inside the driver, yet the folder already tells the truth.
        let manifest = read_manifest(&folder);
        assert_eq!(manifest.status, RunStatus::Cancelled);
        assert!(manifest.finished_at.is_some());
        assert_eq!(manifest.targets.len(), 2);
        assert!(manifest.targets.iter().all(|t| t.status == RunStatus::Cancelled));
        let last = last_event(&folder);
        assert_eq!(last.type_, "run.finished");
        assert_eq!(last.data["status"], "cancelled");

        // When the driver finally lets go, nothing is added after run.finished and run.json stays.
        let late = worker.join().unwrap();
        assert!(late.finished_at.is_some());
    });
    assert_eq!(last_event(&folder).type_, "run.finished");
    assert_eq!(read_manifest(&folder).status, RunStatus::Cancelled);
    assert_eq!(events.iter().filter(|e| e.type_ == "run.finished").count(), 1);
    let leftovers: Vec<_> = fs::read_dir(&folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    fs::remove_dir_all(folder).unwrap();
}

#[test]
fn a_run_that_ends_by_itself_does_not_wait_for_an_abort() {
    let started = Instant::now();
    let (manifest, _, folder) = run_dying(None);
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(manifest.status, RunStatus::Cancelled);
    fs::remove_dir_all(folder).unwrap();
}
