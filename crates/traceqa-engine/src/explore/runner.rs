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
use crate::extract::{PageRead, Progress, Sink};
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
    /// How pages are carried out of the browser (see [`PageRead`]).
    pub page_read: PageRead,
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
                )
                .with_page_read(request.page_read);
                let explored = explore(&mut browser, &request.options, cancel, &mut |event| {
                    progress.emit(event.kind(), None, redact_text(&event.message()), event.data());
                });
                super::tell_page_read_check(&browser, request.page_read, &progress);
                browser.close();
                explored
            });
            done.store(true, Ordering::SeqCst);
            result
        });

        let (status, explored) = match result {
            Ok(explored) => {
                let status = match explored.graph.stopped {
                    Stop::Cancelled => RunStatus::Cancelled,
                    Stop::Error => RunStatus::Failed,
                    _ => RunStatus::Completed,
                };
                write_results(&folder, &explored, &progress);
                (status, Some(explored))
            }
            Err(message) => {
                let text: String = redact_text(&message).chars().take(200).collect();
                eprintln!("traceqa: internal error: {text}");
                progress.emit("error", None, format!("internal error: {text}"), json!({}));
                (RunStatus::Failed, None)
            }
        };
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

/// Writes `graph.json` and `coverage.json`, and tells of the error that ended the walk, if one did.
fn write_results(folder: &Path, explored: &Explored, progress: &Progress<'_, '_>) {
    for (name, bytes) in [
        ("graph.json", explored.graph.to_bytes()),
        ("coverage.json", explored.coverage.to_bytes()),
    ] {
        if let Err(error) = write_file(folder, name, &bytes) {
            eprintln!("traceqa: could not write {name}: {error}");
        }
    }
    if let Some(message) = &explored.error {
        eprintln!("traceqa: {}", redact_text(message));
        progress.emit("error", None, redact_text(message), json!({}));
    }
}

/// Writes a file of the run folder so that a reader never sees half of it.
pub(super) fn write_file(folder: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let temporary = folder.join(format!("{name}.tmp"));
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, folder.join(name))
}

/// Waits for an abort request while the exploration goes on. On a request it writes the final `run.json` itself,
/// because a browser that is stuck will not let the walk finish.
pub(super) fn watch_for_abort(
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
