//! One replay as a run: the folder, the events, the abort watcher, and `replay.json` at the end.
//!
//! The same shape as `runner.rs` (it shares the watcher and the file writer): `run.json` (feature `replay`, no
//! targets), `events.jsonl` and `replay.json`. The run is closed whatever happens: a failure, a cancel, a second signal
//! that cannot wait for the browser, a panic.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;

use serde_json::json;
use traceqa_common::supervise::guard;
use traceqa_common::sync::LockExt;
use traceqa_common::types::{BrowserTarget, Event, RunManifest, RunStatus, StorageState, Viewport};
use traceqa_recorder::{new_run_id, now_rfc3339, RunWriter};

use super::replay::{replay, Ended, Mode, Options, Replayed};
use super::replay_doc::Doc;
use super::runner::{watch_for_abort, write_file};
use super::{Live, DEFAULT_LOAD_TIMEOUT_MS};
use crate::driver::{DriverCommand, Drivers};
use crate::extract::{PageRead, Progress, Sink};
use crate::run::{save, Abort, EngineError, RUN_FOLDER_VERSION};
use crate::url_safety::{check_navigable, redact, redact_text};

/// The `feature` of a replay's `run.json`.
pub const FEATURE: &str = "replay";

/// Everything a replay needs.
pub struct Request {
    pub doc: Doc,
    pub mode: Mode,
    pub options: Options,
    pub target: BrowserTarget,
    pub viewport: Viewport,
    pub storage_state: Option<StorageState>,
    pub out_dir: PathBuf,
    pub drivers: Vec<DriverCommand>,
    /// How pages are carried out of the browser (see [`PageRead`]).
    pub page_read: PageRead,
}

/// A replay that has its folder and has not started.
pub struct ReplayRun {
    request: Request,
    writer: RunWriter,
}

/// What a replay came to.
pub struct Finished {
    pub manifest: RunManifest,
    pub replayed: Option<Replayed>,
}

impl ReplayRun {
    /// Checks the graph and the address, and creates the run folder.
    pub fn prepare(request: Request) -> Result<Self, EngineError> {
        check_navigable(&request.doc.start_url).map_err(EngineError::Invalid)?;
        request
            .doc
            .screens()
            .map_err(|error| EngineError::Invalid(error.to_string()))?;
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
            url: redact(&request.doc.start_url),
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
                    format!("Replaying {}", manifest.url),
                    json!({ "browser": request.target.browser, "mode": request.mode.as_str() }),
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
                let replayed = replay(
                    &mut browser,
                    &request.doc,
                    &request.mode,
                    &request.options,
                    cancel,
                    &mut |event| progress.emit(event.kind(), None, redact_text(&event.message()), event.data()),
                );
                super::tell_page_read_check(&browser, request.page_read, &progress);
                browser.close();
                replayed
            });
            done.store(true, Ordering::SeqCst);
            result
        });

        let (status, replayed) = match result {
            Ok(replayed) => {
                let status = match replayed.ended {
                    Ended::Cancelled => RunStatus::Cancelled,
                    Ended::Error => RunStatus::Failed,
                    _ => RunStatus::Completed,
                };
                write_results(&folder, &request, &replayed, &progress);
                (status, Some(replayed))
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
        Finished { manifest, replayed }
    }
}

/// Writes `replay.json`, and tells of the error that ended the replay, if one did.
fn write_results(folder: &Path, request: &Request, replayed: &Replayed, progress: &Progress<'_, '_>) {
    let bytes = replayed.to_bytes(&request.doc, &request.mode);
    if let Err(error) = write_file(folder, "replay.json", &bytes) {
        eprintln!("traceqa: could not write replay.json: {error}");
    }
    if let Some(message) = &replayed.error {
        eprintln!("traceqa: {}", redact_text(message));
        progress.emit("error", None, redact_text(message), json!({}));
    }
}
