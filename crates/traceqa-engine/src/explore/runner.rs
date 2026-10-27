//! One exploration as a run: the folder, the events, the abort watcher, and the files at the end.
//!
//! The folder holds `run.json` (feature `explore`, no targets: there is one graph and not one reading per browser),
//! `events.jsonl` and `graph.json`. Like every run, it is closed whatever happens: a failure, a cancel, a second
//! signal that cannot wait for the browser, a panic.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Instant;

use serde_json::json;
use traceqa_common::supervise::guard;
use traceqa_common::sync::LockExt;
use traceqa_common::types::{BrowserTarget, Event, RunManifest, RunStatus, StorageState, Viewport};
use traceqa_recorder::{new_run_id, now_rfc3339, write_manifest_in, RunWriter};

use super::walk::{explore, Explored};
use super::{Live, Options, Stop, DEFAULT_LOAD_TIMEOUT_MS};
use crate::driver::{DriverCommand, Drivers};
use crate::extract::{Progress, Sink};
use crate::run::{lock_before, save, Abort, EngineError, ABORT_LOCK_WAIT, ABORT_POLL, RUN_FOLDER_VERSION};
use crate::url_safety::{check_navigable, redact, redact_text};

/// The `feature` of an exploration's `run.json`.
pub const FEATURE: &str = "explore";

/// Everything an exploration needs besides the walk's own options.
pub struct Request {
    pub options: Options,
    pub target: BrowserTarget,
    pub viewport: Viewport,
    pub storage_state: Option<StorageState>,
    pub out_dir: PathBuf,
    pub drivers: Vec<DriverCommand>,
}

/// An exploration that has its folder and has not started.
pub struct ExploreRun {
    request: Request,
    writer: RunWriter,
}

/// What an exploration came to.
pub struct Finished {
    pub manifest: RunManifest,
    pub explored: Option<Explored>,
}

impl ExploreRun {
    /// Checks the address and creates the run folder.
    pub fn prepare(request: Request) -> Result<Self, EngineError> {
        check_navigable(&request.options.start_url).map_err(EngineError::Invalid)?;
        let mut attempts = 0;
        let writer = loop {
            match RunWriter::create(&request.out_dir, &new_run_id()) {
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && attempts < 20 => attempts += 1,
                other => break other?,
            }
        };
        Ok(Self { request, writer })
    }

    pub fn folder(&self) -> &Path {
        self.writer.dir()
    }

    pub fn execute(self, cancel: &AtomicBool, abort: &Abort, listener: &mut (dyn FnMut(&Event) + Send)) -> Finished {
        let Self { request, writer } = self;
        let mut manifest = RunManifest {
            format_version: RUN_FOLDER_VERSION,
            run_id: writer.run_id().to_string(),
            feature: FEATURE.to_string(),
            url: redact(&request.options.start_url),
            status: RunStatus::Running,
            started_at: now_rfc3339(),
            finished_at: None,
            engine_version: crate::VERSION.to_string(),
            targets: Vec::new(),
        };
        save(&writer, &manifest);
        let folder = writer.dir().to_path_buf();
        let sink = Mutex::new(Sink {
            writer,
            listener,
            sealed: false,
        });
        let progress = Progress { sink: &sink };
        let done = AtomicBool::new(false);
        let registration = abort.register();

        let result = thread::scope(|scope| {
            scope.spawn(|| {
                // A safety net: it must not take the run down with it.
                let _ = guard(|| watch_for_abort(abort, &registration, &done, &sink, &manifest, &folder));
            });
            let result = guard(|| {
                progress.emit(
                    "run.started",
                    None,
                    format!("Exploring {}", manifest.url),
                    json!({ "browser": request.target.browser, "strategy": request.options.strategy.as_str() }),
                );
                let mut drivers = Drivers::new(request.drivers.clone());
                let mut browser = Live::new(
                    &mut drivers,
                    request.target.clone(),
                    request.viewport.clone(),
                    request.storage_state.clone(),
                    request.options.rules.driver_policy(),
                    cancel,
                    DEFAULT_LOAD_TIMEOUT_MS,
                );
                let explored = explore(&mut browser, &request.options, cancel, &mut |line| {
                    progress.emit("progress", None, redact_text(line), json!({}));
                });
                browser.close();
                for call in &explored.model_calls {
                    progress.emit("model.call", None, redact_text(&call.summary()), call.data());
                }
                explored
            });
            done.store(true, Ordering::SeqCst);
            result
        });

        let (status, explored, internal_error) = match result {
            Ok(explored) => {
                let status = match explored.graph.stopped {
                    Stop::Cancelled => RunStatus::Cancelled,
                    Stop::Error => RunStatus::Failed,
                    _ => RunStatus::Completed,
                };
                (status, Some(explored), None)
            }
            Err(message) => (RunStatus::Failed, None, Some(message)),
        };
        if let Some(explored) = &explored {
            if let Err(error) = write_graph(&folder, &explored.graph.to_bytes()) {
                eprintln!("traceqa: could not write graph.json: {error}");
            }
            if let Some(message) = &explored.error {
                eprintln!("traceqa: {}", redact_text(message));
                progress.emit("error", None, redact_text(message), json!({}));
            }
        }
        if let Some(message) = &internal_error {
            let text: String = redact_text(message).chars().take(200).collect();
            eprintln!("traceqa: internal error: {text}");
            progress.emit("error", None, format!("internal error: {text}"), json!({}));
        }
        manifest.status = status;
        manifest.finished_at = Some(now_rfc3339());
        progress.emit(
            "run.finished",
            None,
            format!("Run {}", manifest.status),
            json!({ "status": manifest.status }),
        );
        let sink = sink.into_inner_ok();
        // After an abort the watcher has written the final run.json; a late write must not undo it.
        if !sink.sealed {
            save(&sink.writer, &manifest);
        }
        registration.release();
        Finished { manifest, explored }
    }
}

/// Writes `graph.json` so that a reader never sees half of it.
fn write_graph(folder: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temporary = folder.join("graph.json.tmp");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, folder.join("graph.json"))
}

/// Waits for an abort request while the exploration goes on. On a request it writes the final `run.json` itself,
/// because a browser that is stuck will not let the walk finish.
fn watch_for_abort(
    abort: &Abort,
    registration: &crate::run::Registration<'_>,
    done: &AtomicBool,
    sink: &Mutex<Sink<'_>>,
    base: &RunManifest,
    folder: &Path,
) {
    while !abort.is_requested() {
        if done.load(Ordering::SeqCst) {
            return;
        }
        thread::sleep(ABORT_POLL);
    }
    let deadline = Instant::now() + ABORT_LOCK_WAIT;
    let mut manifest = base.clone();
    manifest.status = RunStatus::Cancelled;
    manifest.finished_at = Some(now_rfc3339());
    match lock_before(sink, deadline) {
        Some(mut guard) => {
            let sink = &mut *guard;
            if !sink.sealed {
                let message = format!("Run {}", manifest.status);
                match sink
                    .writer
                    .event("run.finished", None, message, json!({ "status": manifest.status }))
                {
                    Ok(event) => (sink.listener)(&event),
                    Err(error) => eprintln!("traceqa: could not write events.jsonl: {error}"),
                }
                sink.sealed = true;
                save(&sink.writer, &manifest);
            }
        }
        // The recorder is held by a stuck thread: run.json still must not stay `running`.
        None => {
            if let Err(error) = write_manifest_in(folder, &manifest) {
                eprintln!("traceqa: could not write run.json: {error}");
            }
        }
    }
    registration.release();
}
