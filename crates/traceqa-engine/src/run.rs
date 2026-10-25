//! One run: every requested browser at every requested viewport.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, TryLockError};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use thiserror::Error;
use traceqa_protocol::supervise::guard;
use traceqa_protocol::sync::{CondvarExt, LockExt};
use traceqa_protocol::types::{BrowserTarget, Event, RunManifest, RunStartParams, RunStatus, TargetSummary, Viewport};
use traceqa_recorder::{new_run_id, now_rfc3339, write_manifest_in, RunWriter};

use crate::driver::{DriverCommand, DriverError, Drivers, ErrorKind};
use crate::extract::axe::AxeScript;
use crate::extract::{extract_target, Job, Progress, Sink};
use crate::url_policy::UrlPolicy;
use crate::url_safety::{check_navigable, redact};

pub(crate) const RUN_FOLDER_VERSION: u64 = traceqa_protocol::contracts::run_folder::VERSION;

/// Features this engine can run.
pub(crate) const FEATURES: [&str; 1] = ["extract"];

/// Drivers used when none are configured: from PATH, else the copies `traceqa install` made.
const DEFAULT_DRIVERS: [&str; 2] = ["traceqa-driver-playwright", "traceqa-driver-webdriver"];

/// Environment variable holding driver command lines separated by `;`.
pub const DRIVERS_ENV: &str = "TRACEQA_DRIVERS";

/// Why a run, or one target of it, did not go through. What `Display` says is what the CLI and
/// the MCP server show.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum EngineError {
    /// The run was stopped on request.
    #[error("cancelled")]
    Cancelled,
    /// A driver failed, or could not be reached. A `timeout` kind means the browser or its
    /// driver did not answer in time.
    #[error(transparent)]
    Driver(DriverError),
    /// A file or folder of the run could not be read or written.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The request or the page was not acceptable; the text says why.
    #[error("{0}")]
    Invalid(String),
}

impl From<DriverError> for EngineError {
    /// A driver call that was given up on because of a cancel is a cancelled run, not a failure.
    fn from(error: DriverError) -> Self {
        if error.is_cancelled() {
            Self::Cancelled
        } else {
            Self::Driver(error)
        }
    }
}

impl EngineError {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    /// True when the run was stopped on request.
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    /// True when the browser or its driver did not answer in time.
    pub fn is_timed_out(&self) -> bool {
        matches!(self, Self::Driver(error) if *error.kind() == ErrorKind::Timeout)
    }
}

/// The driver commands to use: `explicit` if given, else `TRACEQA_DRIVERS`, else the defaults.
pub fn driver_commands(explicit: &[String]) -> Vec<DriverCommand> {
    let from_env = std::env::var(DRIVERS_ENV).unwrap_or_default();
    let lines: Vec<&str> = if !explicit.is_empty() {
        explicit.iter().map(String::as_str).collect()
    } else if !from_env.trim().is_empty() {
        from_env.split(';').collect()
    } else {
        return DEFAULT_DRIVERS.iter().map(|name| default_driver(name)).collect();
    };
    lines.into_iter().filter_map(DriverCommand::parse).collect()
}

/// A default driver: the one on PATH, else the installed one. When there is neither, the bare
/// name is kept, so the error that follows names the driver that is missing.
fn default_driver(name: &str) -> DriverCommand {
    let program = match crate::home::on_path(name) {
        Some(_) => name.to_string(),
        None => crate::home::installed(name).map_or_else(|| name.to_string(), |path| path.display().to_string()),
    };
    DriverCommand {
        program,
        args: Vec::new(),
    }
}

/// A run that has been accepted and has its folder, but has not started.
pub struct Run {
    params: RunStartParams,
    writer: RunWriter,
    /// axe-core's source, read before the run folder was made, when the request asked for it.
    axe: Option<AxeScript>,
}

impl Run {
    /// Checks the request and creates the run folder.
    pub fn prepare(params: RunStartParams) -> Result<Self, EngineError> {
        if !FEATURES.contains(&params.feature.as_str()) {
            return Err(EngineError::invalid(format!(
                "unknown feature {:?}. This engine runs: {}",
                params.feature,
                FEATURES.join(", ")
            )));
        }
        check_navigable(&params.url).map_err(EngineError::Invalid)?;
        if params.targets.is_empty() || params.viewports.is_empty() {
            return Err(EngineError::invalid("a run needs at least one target and one viewport"));
        }
        // Before the run folder exists, so a refused request leaves nothing behind. A huge size
        // would otherwise only end when the driver's open timed out.
        for viewport in &params.viewports {
            viewport
                .check()
                .map_err(|error| EngineError::invalid(error.to_string()))?;
        }
        // axe-core is not shipped, so a request for it is refused here, before the run folder exists,
        // when it is not on this machine.
        let axe = if params.options.as_ref().and_then(|o| o.axe).unwrap_or(false) {
            Some(AxeScript::load().map_err(EngineError::Invalid)?)
        } else {
            None
        };
        let out_dir = PathBuf::from(params.out_dir.as_deref().unwrap_or("runs"));
        // An id that is already taken on disk (another process, same second) is simply drawn again.
        let mut attempts = 0;
        let writer = loop {
            match RunWriter::create(&out_dir, &new_run_id()) {
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && attempts < 20 => attempts += 1,
                other => break other?,
            }
        };
        Ok(Self { params, writer, axe })
    }

    pub fn id(&self) -> &str {
        self.writer.run_id()
    }

    pub fn folder(&self) -> &Path {
        self.writer.dir()
    }

    /// Runs every target and returns the final manifest, which is also in `run.json`.
    ///
    /// Targets run side by side, as many at a time as the request or the machine allows. A
    /// target that fails does not stop the others. Events from all of them reach `listener`
    /// one at a time, in the order they are written to `events.jsonl`.
    pub fn execute(self, execution: &Execution, listener: &mut (dyn FnMut(&Event) + Send)) -> RunManifest {
        self.execute_abortable(execution, &Abort::new(), listener)
    }

    /// As [`execute`](Self::execute), and `abort` can end the run at once. When it is requested,
    /// a watcher writes the final `run.json` (status `cancelled`, targets that had not finished
    /// cancelled) and the closing `run.finished` event within about a second, without waiting
    /// for the drivers, and then closes `abort`. The caller then exits. Use it where the person
    /// can insist on quitting while a driver does not answer. A caller that then exits should call
    /// [`kill_all_drivers`](crate::driver::kill_all_drivers) first: exiting runs no `Drop`, so a
    /// driver that ignores its input would otherwise outlive the command.
    pub fn execute_abortable(
        self,
        execution: &Execution,
        abort: &Abort,
        listener: &mut (dyn FnMut(&Event) + Send),
    ) -> RunManifest {
        let Self { params, writer, axe } = self;
        let run_id = writer.run_id().to_string();
        let mut manifest = RunManifest {
            format_version: RUN_FOLDER_VERSION,
            run_id: run_id.clone(),
            feature: params.feature.clone(),
            url: redact(&params.url),
            status: RunStatus::Running,
            started_at: now_rfc3339(),
            finished_at: None,
            engine_version: crate::VERSION.to_string(),
            targets: Vec::new(),
        };
        save(&writer, &manifest);

        let (jobs, workers) = plan(&params);

        let folder = writer.dir().to_path_buf();
        let sink = Mutex::new(Sink {
            writer,
            listener,
            sealed: false,
        });
        let progress = Progress { sink: &sink };
        // Filled in as targets end, so what finished is not lost if something panics.
        let finished: Mutex<Vec<Option<Finished>>> = Mutex::new(jobs.iter().map(|_| None).collect());

        let done = AtomicBool::new(false);
        let registration = abort.register();
        let supervised = thread::scope(|scope| {
            scope.spawn(|| {
                let watched = Watched {
                    abort,
                    registration: &registration,
                    done: &done,
                    sink: &sink,
                    finished: &finished,
                    jobs: &jobs,
                    base: &manifest,
                    folder: &folder,
                };
                // The watcher is a safety net: it must not take the run down with it.
                let _ = guard(|| watch_for_abort(&watched));
            });
            // A panic anywhere in the run (a worker, a driver) must still leave a finished run folder
            // and an answer for the caller, so the work is supervised.
            let supervised = guard(|| {
                progress.emit(
                    "run.started",
                    None,
                    format!(
                        "Extracting {} in {} browser and viewport combinations, {workers} at a time",
                        manifest.url,
                        jobs.len()
                    ),
                    json!({ "targets": jobs.len(), "parallel": workers }),
                );

                let exclusive = Exclusive::default();
                let shared = Shared {
                    params: &params,
                    run_id: &run_id,
                    progress,
                    exclusive: &exclusive,
                    cancel: execution.cancel,
                    policy: execution.policy,
                    axe: axe.as_ref(),
                };
                let worker_panic = run_workers(&jobs, workers, &shared, execution, &finished);
                retry_timed_out(&jobs, &finished, workers, &shared, execution);
                worker_panic
            });
            done.store(true, Ordering::SeqCst);
            supervised
        });

        let finished = finished.into_inner_ok();
        // The first panic wins: a worker's own words, else the supervisor's.
        let internal_error = match supervised {
            Ok(None) => None,
            Ok(Some(message)) | Err(message) => Some(internal_error_text(&message)),
        };
        if let Some(text) = &internal_error {
            eprintln!("traceqa: run {run_id}: {text}");
            progress.emit("error", None, format!("Run failed: {text}"), json!({}));
        }
        // In the order they were asked for, whatever order they finished in. A target that never
        // finished, because the run panicked, failed.
        let mut ids = Vec::with_capacity(finished.len());
        manifest.targets = jobs
            .iter()
            .zip(finished)
            .map(|(job, outcome)| match outcome {
                Some(outcome) => {
                    ids.push(outcome.ids);
                    outcome.summary
                }
                None => {
                    ids.push(Vec::new());
                    unfinished_summary(job, internal_error.as_deref())
                }
            })
            .collect();

        for (message, data) in disagreements(&manifest.targets, &ids) {
            progress.emit("warning", None, message, data);
        }

        manifest.status = roll_up_status(&manifest.targets);
        if internal_error.is_some() {
            manifest.status = RunStatus::Failed;
        }
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
        manifest
    }
}

/// A request to end the runs at once, for the person who presses Ctrl-C a second time, and a
/// count of the runs that are still going. It is a flag and a signal that the run folders have
/// been closed, so it is safe to set from a signal handler thread: the handler only calls
/// [`request`](Self::request), and whoever wants to exit waits for
/// [`wait_closed`](Self::wait_closed) first. One `Abort` may serve several runs (`serve`, `mcp`):
/// it counts as closed only when every run that started under it has written its folder.
#[derive(Debug)]
pub struct Abort {
    requested: AtomicBool,
    counts: Mutex<Counts>,
    changed: Condvar,
}

#[derive(Debug, Default)]
struct Counts {
    /// Runs whose folder is not closed yet (an abort would still write it).
    open: usize,
    /// Runs whose folder has been closed, by an abort or by finishing.
    closed: usize,
    /// Holders of an [`InFlight`] guard: work that must be seen through to its last message.
    in_flight: usize,
}

impl Abort {
    pub const fn new() -> Self {
        Self {
            requested: AtomicBool::new(false),
            counts: Mutex::new(Counts {
                open: 0,
                closed: 0,
                in_flight: 0,
            }),
            changed: Condvar::new(),
        }
    }

    /// Asks the runs to write their final state and stop waiting for the drivers.
    pub fn request(&self) {
        self.requested.store(true, Ordering::SeqCst);
    }

    pub fn is_requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    /// Waits until every run started under this `Abort` has closed its folder, and at least one
    /// did, for at most `timeout`. Returns whether that happened.
    pub fn wait_closed(&self, timeout: Duration) -> bool {
        self.wait_until(timeout, |counts| counts.open == 0 && counts.closed > 0)
    }

    /// Like [`wait_closed`](Self::wait_closed), but also true when no run ever started.
    pub fn wait_quiet(&self, timeout: Duration) -> bool {
        self.wait_until(timeout, |counts| counts.open == 0)
    }

    /// Marks work as in flight until the guard drops. A run thread holds it until it has sent its
    /// last message to the client, so [`wait_idle`](Self::wait_idle) means everything is out.
    pub fn enter(&self) -> InFlight<'_> {
        self.counts.lock_ok().in_flight += 1;
        InFlight { abort: self }
    }

    /// Waits until no [`InFlight`] guard is held, for at most `timeout`. Returns whether none is.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        self.wait_until(timeout, |counts| counts.in_flight == 0)
    }

    fn wait_until(&self, timeout: Duration, ready: impl Fn(&Counts) -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        let mut counts = self.counts.lock_ok();
        while !ready(&counts) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            counts = self.changed.wait_timeout_ok(counts, left);
        }
        true
    }

    /// A run under this `Abort` starts; its folder counts as open until the registration is released.
    fn register(&self) -> Registration<'_> {
        self.counts.lock_ok().open += 1;
        Registration {
            abort: self,
            released: AtomicBool::new(false),
        }
    }
}

impl Default for Abort {
    fn default() -> Self {
        Self::new()
    }
}

/// What a long-lived server (`serve`, `mcp`) is given by the process's signal handler: a flag
/// the first signal sets (cancel every run, accept no new one) and the [`Abort`] the second one
/// requests. Both live as long as the process.
#[derive(Clone, Copy, Debug)]
pub struct Interrupt {
    pub cancel: &'static AtomicBool,
    pub abort: &'static Abort,
}

/// Work in flight under an [`Abort`]; see [`Abort::enter`].
pub struct InFlight<'a> {
    abort: &'a Abort,
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.abort.counts.lock_ok().in_flight -= 1;
        self.abort.changed.notify_all();
    }
}

/// One run's claim on an [`Abort`]: released when its folder is closed, by the abort watcher or
/// by the run finishing, whichever comes first.
struct Registration<'a> {
    abort: &'a Abort,
    released: AtomicBool,
}

impl Registration<'_> {
    fn release(&self) {
        if !self.released.swap(true, Ordering::SeqCst) {
            let mut counts = self.abort.counts.lock_ok();
            counts.open -= 1;
            counts.closed += 1;
            drop(counts);
            self.abort.changed.notify_all();
        }
    }
}

impl Drop for Registration<'_> {
    fn drop(&mut self) {
        self.release();
    }
}

/// How often the watcher looks for an abort request.
const ABORT_POLL: Duration = Duration::from_millis(20);

/// How long the watcher waits for the run's locks before it writes without them.
const ABORT_LOCK_WAIT: Duration = Duration::from_secs(1);

/// What the abort watcher needs to see of the run.
struct Watched<'a, 'b> {
    abort: &'a Abort,
    registration: &'a Registration<'a>,
    done: &'a AtomicBool,
    sink: &'a Mutex<Sink<'b>>,
    finished: &'a Mutex<Vec<Option<Finished>>>,
    jobs: &'a [Planned<'a>],
    base: &'a RunManifest,
    folder: &'a Path,
}

/// Takes the lock if it is free before `deadline`, polling. A thread that is stuck inside a
/// driver call may hold a lock for long, and the watcher must never wait for it.
fn lock_before<T>(mutex: &Mutex<T>, deadline: Instant) -> Option<MutexGuard<'_, T>> {
    loop {
        match mutex.try_lock() {
            Ok(guard) => return Some(guard),
            Err(TryLockError::Poisoned(poisoned)) => return Some(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return None;
                }
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

/// Waits for an abort request while the run goes on. On a request it writes the final state
/// itself, from what has finished so far, because the targets that are stuck will not.
fn watch_for_abort(watched: &Watched) {
    while !watched.abort.is_requested() {
        if watched.done.load(Ordering::SeqCst) {
            return;
        }
        thread::sleep(ABORT_POLL);
    }
    let deadline = Instant::now() + ABORT_LOCK_WAIT;
    let mut manifest = watched.base.clone();
    {
        let finished = lock_before(watched.finished, deadline);
        manifest.targets = watched
            .jobs
            .iter()
            .enumerate()
            .map(|(index, job)| {
                finished
                    .as_ref()
                    .and_then(|done| done[index].as_ref())
                    .map_or_else(|| cancelled_summary(job), |outcome| outcome.summary.clone())
            })
            .collect();
    }
    manifest.status = RunStatus::Cancelled;
    manifest.finished_at = Some(now_rfc3339());
    match lock_before(watched.sink, deadline) {
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
            if let Err(error) = write_manifest_in(watched.folder, &manifest) {
                eprintln!("traceqa: could not write run.json: {error}");
            }
        }
    }
    watched.registration.release();
}

/// What a failed run says when it panicked. The text is bounded, and is the only place the
/// panic's own words reach a run folder.
fn internal_error_text(message: &str) -> String {
    // Redacted first, then cut: a cut cannot leave the start of a secret behind.
    let safe = crate::url_safety::redact_text(message);
    let short: String = safe.chars().take(200).collect();
    format!("internal error: {short}")
}

/// The manifest of a run whose supervisor itself panicked, so `execute` never returned one (a
/// listener that panics while the run folder is being closed). It is `run.json` as last saved,
/// marked failed and written back, so the folder never stays `running`.
pub(crate) fn failed_after_panic(folder: &Path, run_id: &str, message: &str) -> RunManifest {
    let path = folder.join("run.json");
    let mut manifest = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<RunManifest>(&text).ok())
        .unwrap_or_else(|| RunManifest {
            format_version: RUN_FOLDER_VERSION,
            run_id: run_id.to_string(),
            feature: FEATURES[0].to_string(),
            url: String::new(),
            status: RunStatus::Running,
            started_at: now_rfc3339(),
            finished_at: None,
            engine_version: crate::VERSION.to_string(),
            targets: Vec::new(),
        });
    eprintln!("traceqa: run {run_id}: {}", internal_error_text(message));
    manifest.status = RunStatus::Failed;
    manifest.finished_at.get_or_insert_with(now_rfc3339);
    for target in manifest.targets.iter_mut().filter(|t| t.status == RunStatus::Running) {
        target.status = RunStatus::Failed;
        target.error = Some(internal_error_text(message));
    }
    if write_manifest_in(folder, &manifest).is_err() {
        eprintln!("traceqa: could not write run.json after a panic");
    }
    manifest
}

/// A target the run was stopped before it finished.
fn cancelled_summary(job: &Planned) -> TargetSummary {
    TargetSummary {
        status: RunStatus::Cancelled,
        error: None,
        ..unfinished_summary(job, None)
    }
}

/// A target that never reached an end: the run was stopped by a panic, or never got to it.
fn unfinished_summary(job: &Planned, internal_error: Option<&str>) -> TargetSummary {
    TargetSummary {
        dir: job.name.clone(),
        target: job.target.clone(),
        viewport: job.viewport.clone(),
        browser: None,
        status: RunStatus::Failed,
        component_count: 0,
        instance_count: 0,
        warning_count: 0,
        duration_ms: 0,
        timings: None,
        error: Some(
            internal_error
                .unwrap_or("internal error: the target did not finish")
                .to_string(),
        ),
    }
}

/// How a run is carried out.
pub struct Execution<'a> {
    /// Makes the drivers one worker uses. It is called once per worker.
    pub drivers: &'a (dyn Fn() -> Drivers + Sync),
    /// Set this to stop the run. Targets that have not started are skipped, and the ones
    /// that are running stop at their next step.
    pub cancel: &'a AtomicBool,
    /// Where browsers may go, when the operator set rules (the MCP server does). A page that
    /// ends up somewhere the rules refuse fails its target and nothing from it is written.
    pub policy: Option<&'a UrlPolicy>,
}

/// One browser at one viewport, and the folder its results go to.
struct Planned<'a> {
    name: String,
    target: &'a BrowserTarget,
    viewport: &'a Viewport,
}

/// What every target of a run has in common.
struct Shared<'a, 'b> {
    params: &'a RunStartParams,
    run_id: &'a str,
    progress: Progress<'a, 'b>,
    exclusive: &'a Exclusive,
    cancel: &'a AtomicBool,
    policy: Option<&'a UrlPolicy>,
    axe: Option<&'a AxeScript>,
}

/// Every browser at every viewport, each with the folder name it will write to, and how many
/// of them to run at once.
fn plan(params: &RunStartParams) -> (Vec<Planned<'_>>, usize) {
    let mut used_names = HashSet::new();
    let jobs: Vec<Planned> = params
        .targets
        .iter()
        .flat_map(|target| params.viewports.iter().map(move |viewport| (target, viewport)))
        .map(|(target, viewport)| Planned {
            name: unique_name(&mut used_names, target, viewport),
            target,
            viewport,
        })
        .collect();
    let workers = match params.parallel {
        Some(asked) if asked > 0 => asked as usize,
        _ => default_parallelism(),
    }
    .min(jobs.len());
    (jobs, workers)
}

/// Runs the jobs on `workers` threads, each with drivers of its own, and records how each went
/// in `finished`, in the order of `jobs`. A worker that panics stops alone: the others go on, and
/// what finished stays in `finished`. Returns the message of the first panic, if there was one.
fn run_workers(
    jobs: &[Planned],
    workers: usize,
    shared: &Shared,
    execution: &Execution,
    finished: &Mutex<Vec<Option<Finished>>>,
) -> Option<String> {
    let next = AtomicUsize::new(0);
    let panicked: Mutex<Option<String>> = Mutex::new(None);
    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                let worked = guard(|| {
                    // A driver process serves one browser at a time, so each worker has its own.
                    let mut drivers = (execution.drivers)();
                    while let Some((index, job)) = claim(&next, jobs) {
                        let outcome = run_target(job, shared, &mut drivers);
                        finished.lock_ok()[index] = Some(outcome);
                    }
                });
                if let Err(message) = worked {
                    panicked.lock_ok().get_or_insert(message);
                }
            });
        }
    });
    panicked.into_inner_ok()
}

/// The targets to run a second time: only when several ran at once, and only those that failed
/// because the browser or its driver did not answer in time.
fn timed_out_targets(workers: usize, finished: &[Option<Finished>]) -> Vec<usize> {
    if workers <= 1 {
        return Vec::new();
    }
    (0..finished.len())
        .filter(|&index| finished[index].as_ref().is_some_and(|outcome| outcome.timed_out))
        .collect()
}

/// A page that is heavy to render can time out only because several browsers were rendering it
/// at once. Such a target gets a second go with the machine to itself.
fn retry_timed_out(
    jobs: &[Planned],
    finished: &Mutex<Vec<Option<Finished>>>,
    workers: usize,
    shared: &Shared,
    execution: &Execution,
) {
    let again = timed_out_targets(workers, &finished.lock_ok());
    for index in again {
        if execution.cancel.load(Ordering::Relaxed) {
            break;
        }
        let job = &jobs[index];
        shared.progress.emit(
            "target.retried",
            Some(&job.name),
            format!(
                "Trying {} again on its own: it timed out while other browsers were running",
                job.name
            ),
            json!({}),
        );
        let outcome = run_target(job, shared, &mut (execution.drivers)());
        finished.lock_ok()[index] = Some(outcome);
    }
}

/// The run is cancelled if any target was, else failed if any failed, else completed.
fn roll_up_status(targets: &[TargetSummary]) -> RunStatus {
    let has = |status| targets.iter().any(|target| target.status == status);
    if has(RunStatus::Cancelled) {
        RunStatus::Cancelled
    } else if has(RunStatus::Failed) {
        RunStatus::Failed
    } else {
        RunStatus::Completed
    }
}

/// How many browsers to run at once when the request does not say. Each one is a browser
/// process that renders a page, so half the cores is a safe share, and beyond four the gain
/// is small while the memory use is not.
fn default_parallelism() -> usize {
    let cores = thread::available_parallelism().map_or(1, std::num::NonZero::get);
    (cores / 2).clamp(1, 4)
}

/// Hands out the next job that no worker has taken.
fn claim<'a, 'b>(next: &AtomicUsize, jobs: &'a [Planned<'b>]) -> Option<(usize, &'a Planned<'b>)> {
    let index = next.fetch_add(1, Ordering::Relaxed);
    jobs.get(index).map(|job| (index, job))
}

/// How one target went.
struct Finished {
    summary: TargetSummary,
    /// It failed because the browser or its driver did not answer in time.
    timed_out: bool,
    /// Component ids found, sorted. Kept so the run can say which ids browsers disagree on.
    /// Empty unless the target completed.
    ids: Vec<String>,
}

/// Runs one target and reports how it went. It never fails: a failure is part of the report.
fn run_target(job: &Planned, shared: &Shared, drivers: &mut Drivers) -> Finished {
    let name = job.name.as_str();
    let options = shared.params.options.as_ref();
    let progress = shared.progress;
    let mut summary = TargetSummary {
        dir: job.name.clone(),
        target: job.target.clone(),
        viewport: job.viewport.clone(),
        browser: None,
        status: RunStatus::Cancelled,
        component_count: 0,
        instance_count: 0,
        warning_count: 0,
        duration_ms: 0,
        timings: None,
        error: None,
    };
    if shared.cancel.load(Ordering::Relaxed) {
        return Finished {
            summary,
            timed_out: false,
            ids: Vec::new(),
        };
    }

    progress.emit("target.started", Some(name), format!("Starting {name}"), json!({}));
    let started = Instant::now();
    let outcome = progress
        .with_writer(|writer| writer.target_dir(name))
        .map_err(EngineError::from)
        .and_then(|dir| {
            let display_url = redact(&shared.params.url);
            let extraction = Job {
                run_id: shared.run_id,
                url: &shared.params.url,
                display_url: &display_url,
                target: job.target,
                viewport: job.viewport,
                max_scroll_steps: options.and_then(|o| o.max_scroll_steps),
                timeout_ms: options.and_then(|o| o.timeout_ms),
                crops: options.and_then(|o| o.crops).unwrap_or(false),
                include_hidden: options.and_then(|o| o.include_hidden).unwrap_or(true),
                settle_ms: options.and_then(|o| o.settle_ms).unwrap_or(0),
                name,
                dir: &dir,
                exclusive: shared.exclusive,
                cancel: shared.cancel,
                policy: shared.policy,
                axe: shared.axe,
                storage_state: shared.params.storage_state.as_ref(),
            };
            let (file, browser, timings) = extract_target(drivers, &extraction, progress)?;
            progress.with_writer(|writer| writer.write_components(name, &file))?;
            Ok((file, browser, timings))
        });
    summary.duration_ms = crate::util::ms(started.elapsed());

    let mut ids = Vec::new();
    match outcome {
        Ok((file, browser, timings)) => {
            ids = file.components.iter().map(|c| c.id.clone()).collect();
            ids.sort();
            ids.dedup();
            summary.status = RunStatus::Completed;
            summary.browser = Some(browser);
            summary.component_count = file.components.len() as u64;
            summary.instance_count = file.components.iter().map(|c| c.instances.len() as u64).sum();
            summary.warning_count = file.warnings.len() as u64;
            progress.emit(
                "target.finished",
                Some(name),
                format!(
                    "{name}: {} components, {} instances, {} warnings, {:.1} s",
                    summary.component_count,
                    summary.instance_count,
                    summary.warning_count,
                    summary.duration_ms as f64 / 1000.0
                ),
                json!({
                    "components": summary.component_count,
                    "instances": summary.instance_count,
                    "durationMs": summary.duration_ms,
                    "timings": timings,
                }),
            );
            summary.timings = Some(timings);
        }
        Err(error) if error.is_cancelled() || stopped_by_cancel(&error, shared.cancel, CANCEL_GRACE) => {
            // A cancel that also took the browser or driver down (Ctrl-C reaches all of them)
            // makes them report an error of their own. The target was stopped, not broken: its
            // words stay in the event, not in the summary.
            if !error.is_cancelled() {
                progress.emit(
                    "target.cancelled",
                    Some(name),
                    format!("{name} cancelled: {error}"),
                    json!({}),
                );
            }
        }
        Err(error) => {
            summary.status = RunStatus::Failed;
            summary.error = Some(error.to_string());
            progress.emit("error", Some(name), format!("{name} failed: {error}"), json!({}));
            return Finished {
                summary,
                timed_out: error.is_timed_out(),
                ids: Vec::new(),
            };
        }
    }
    Finished {
        summary,
        timed_out: false,
        ids,
    }
}

/// How long a target whose browser or driver reported a failure waits for the cancel flag.
/// A console Ctrl-C reaches the driver and the process at the same moment, so the driver's
/// error can arrive just before the handler has set the flag.
const CANCEL_GRACE: Duration = Duration::from_millis(300);

/// True when `error` is the aftermath of a cancel: the flag is set, or it is set within `grace`
/// and the error is the kind a dying browser or driver reports. Errors about the page itself
/// (a timeout, a failed navigation, a blocked address) are real failures and do not wait.
fn stopped_by_cancel(error: &EngineError, cancel: &AtomicBool, grace: Duration) -> bool {
    if cancel.load(Ordering::SeqCst) {
        return true;
    }
    let EngineError::Driver(driver) = error else {
        return false;
    };
    let dying = matches!(
        driver.kind(),
        ErrorKind::Transport
            | ErrorKind::Internal
            | ErrorKind::EvalFailed
            | ErrorKind::SessionNotFound
            | ErrorKind::Other(_)
    );
    if !dying {
        return false;
    }
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        if cancel.load(Ordering::SeqCst) {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    cancel.load(Ordering::SeqCst)
}

/// How often a target that waits for a browser looks at the cancel flag.
const CANCEL_SLICE: Duration = Duration::from_millis(250);

/// Browsers of which only one session may exist on the machine. A target that needs one
/// waits here until the target that has it is done.
#[derive(Default)]
pub(crate) struct Exclusive {
    busy: Mutex<HashSet<String>>,
    freed: Condvar,
}

impl Exclusive {
    /// Waits until `browser` is free and takes it until the returned value is dropped. Looks at
    /// `cancel` every [`CANCEL_SLICE`] while it waits and gives up with `None` once it is set.
    pub(crate) fn hold<'a>(&'a self, browser: &str, cancel: &AtomicBool) -> Option<Held<'a>> {
        let mut busy = self.busy.lock_ok();
        while busy.contains(browser) {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            // Woken early by a release, or after a slice; either way the predicate is checked again.
            busy = self.freed.wait_timeout_ok(busy, CANCEL_SLICE);
        }
        busy.insert(browser.to_string());
        Some(Held {
            owner: self,
            browser: browser.to_string(),
        })
    }
}

pub(crate) struct Held<'a> {
    owner: &'a Exclusive,
    browser: String,
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        self.owner.busy.lock_ok().remove(&self.browser);
        self.owner.freed.notify_all();
    }
}

fn save(writer: &RunWriter, manifest: &RunManifest) {
    if let Err(error) = writer.write_manifest(manifest) {
        eprintln!("traceqa: could not write run.json: {error}");
    }
}

/// Ids named in a message; the event data carries up to [`MAX_LISTED_IDS`].
const MESSAGE_IDS: usize = 5;
const MAX_LISTED_IDS: usize = 20;

/// One warning per viewport at which the browsers that finished found different numbers of
/// elements, as a message and the event data. The same page should hold the same controls in
/// every browser, so a difference usually means one of them was read before the page had
/// finished appearing. The message names component ids missing per browser (ids only, never
/// page text); `ids[i]` are the sorted component ids of `targets[i]`. The trigger is the count
/// of instances: browsers that find the same number of different elements raise no warning
/// here, but `traceqa findings` still reports them (rule `browsers-disagree`).
fn disagreements(targets: &[TargetSummary], ids: &[Vec<String>]) -> Vec<(String, Value)> {
    let mut by_viewport: Vec<((u64, u64), Vec<usize>)> = Vec::new();
    for (index, target) in targets.iter().enumerate() {
        if target.status != RunStatus::Completed {
            continue;
        }
        let size = (target.viewport.width, target.viewport.height);
        match by_viewport.iter_mut().find(|(seen, _)| *seen == size) {
            Some((_, group)) => group.push(index),
            None => by_viewport.push((size, vec![index])),
        }
    }
    by_viewport
        .into_iter()
        .filter(|(_, group)| {
            group
                .iter()
                .any(|i| targets[*i].instance_count != targets[group[0]].instance_count)
        })
        .map(|((width, height), group)| {
            let none = Vec::new();
            let ids_of = |i: usize| ids.get(i).unwrap_or(&none);
            let union: BTreeSet<&String> = group.iter().flat_map(|i| ids_of(*i)).collect();
            let found: Vec<String> = group
                .iter()
                .map(|i| format!("{} found {}", targets[*i].target.browser, targets[*i].instance_count))
                .collect();
            let mut counts = serde_json::Map::new();
            let mut missing = serde_json::Map::new();
            let mut missing_total = serde_json::Map::new();
            let mut parts = Vec::new();
            for i in &group {
                let browser = targets[*i].target.browser.clone();
                counts.insert(browser.clone(), json!(targets[*i].instance_count));
                let have: BTreeSet<&String> = ids_of(*i).iter().collect();
                let absent: Vec<&String> = union.iter().filter(|id| !have.contains(*id)).copied().collect();
                if absent.is_empty() {
                    continue;
                }
                let shown: Vec<&str> = absent.iter().take(MESSAGE_IDS).map(|id| id.as_str()).collect();
                let more = absent.len() - shown.len();
                let tail = if more > 0 {
                    format!(" (and {more} more)")
                } else {
                    String::new()
                };
                parts.push(format!("missing in {browser}: {}{tail}", shown.join(", ")));
                missing.insert(
                    browser.clone(),
                    json!(absent.iter().take(MAX_LISTED_IDS).collect::<Vec<_>>()),
                );
                missing_total.insert(browser, json!(absent.len()));
            }
            let detail = if parts.is_empty() {
                String::new()
            } else {
                format!("; {}", parts.join("; "))
            };
            let message = format!(
                "Browsers disagree at {width}x{height}: {}{detail}. The page may not have finished appearing when it was read. Try a settle time.",
                found.join(", ")
            );
            let data = json!({
                "code": "browsers_disagree",
                "viewport": { "width": width, "height": height },
                "counts": counts,
                "missing": missing,
                "missingTotal": missing_total,
            });
            (message, data)
        })
        .collect()
}

/// `firefox-1280x800`, with a number added when the same combination is asked for twice.
fn unique_name(used: &mut HashSet<String>, target: &BrowserTarget, viewport: &Viewport) -> String {
    let browser: String = target
        .browser
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let base = format!("{browser}-{}x{}", viewport.width, viewport.height);
    let mut name = base.clone();
    let mut copy = 2;
    while !used.insert(name.clone()) {
        name = format!("{base}-{copy}");
        copy += 1;
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn request(feature: &str, url: &str, targets: usize) -> RunStartParams {
        RunStartParams {
            feature: feature.into(),
            url: url.into(),
            targets: vec![BrowserTarget::named("firefox"); targets],
            viewports: vec![Viewport {
                width: 1280,
                height: 800,
            }],
            out_dir: Some(
                std::env::temp_dir()
                    .join("traceqa-run-tests")
                    .to_string_lossy()
                    .into_owned(),
            ),
            parallel: None,
            options: None,
            storage_state: None,
        }
    }

    #[test]
    fn prepare_refuses_a_zero_or_huge_viewport_before_creating_the_run_folder() {
        for (width, height) in [(0, 800), (800, 0), (0, 0), (99_999, 99_999), (10_001, 800)] {
            let out =
                std::env::temp_dir().join(format!("traceqa-run-viewport-{width}x{height}-{}", std::process::id()));
            let mut bad = request("extract", "https://shop.test", 1);
            bad.viewports = vec![Viewport { width, height }];
            bad.out_dir = Some(out.to_string_lossy().into_owned());
            let error = Run::prepare(bad).err().expect("should be refused");
            assert!(matches!(error, EngineError::Invalid(_)), "{error:?}");
            assert!(
                error.to_string().contains(&format!("viewport {width}x{height}")),
                "{error}"
            );
            assert!(!out.exists(), "no run folder may be left behind for {width}x{height}");
        }
    }

    #[test]
    fn bad_requests_are_refused_before_anything_is_written() {
        for (bad, expected) in [
            (request("sweep", "https://shop.test", 1), "unknown feature"),
            (request("extract", "shop.test", 1), "http://"),
            (request("extract", "file:///etc/passwd", 1), "http://"),
            (request("extract", "https://shop.test", 0), "at least one target"),
        ] {
            let error = Run::prepare(bad).err().expect("should be refused").to_string();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn browsers_that_find_different_numbers_are_reported_per_viewport() {
        let summary = |browser: &str, width: u64, found: u64, status: RunStatus| TargetSummary {
            dir: format!("{browser}-{width}x800"),
            target: BrowserTarget::named(browser),
            viewport: Viewport { width, height: 800 },
            browser: None,
            status,
            component_count: found,
            instance_count: found,
            warning_count: 0,
            duration_ms: 0,
            timings: None,
            error: None,
        };
        let targets = [
            summary("chromium", 1280, 72, RunStatus::Completed),
            summary("firefox", 1280, 72, RunStatus::Completed),
            summary("chromium", 390, 81, RunStatus::Completed),
            summary("firefox", 390, 54, RunStatus::Completed),
            // A browser that failed found nothing, which is not a disagreement.
            summary("webkit", 1280, 0, RunStatus::Failed),
        ];
        let messages = disagreements(&targets, &[]);
        assert_eq!(messages.len(), 1);
        assert!(
            messages[0].0.contains("390x800: chromium found 81, firefox found 54"),
            "{}",
            messages[0].0
        );
    }

    #[test]
    fn the_warning_names_the_ids_missing_per_browser_and_caps_the_lists() {
        let summary = |browser: &str, found: u64| TargetSummary {
            dir: format!("{browser}-1280x800"),
            target: BrowserTarget::named(browser),
            viewport: Viewport {
                width: 1280,
                height: 800,
            },
            browser: None,
            status: RunStatus::Completed,
            component_count: found,
            instance_count: found,
            warning_count: 0,
            duration_ms: 0,
            timings: None,
            error: None,
        };
        let everything: Vec<String> = (0..30).map(|n| format!("c_{n:012}")).collect();
        let targets = [summary("chromium", 30), summary("firefox", 6)];
        let ids = [everything.clone(), everything[..6].to_vec()];
        let out = disagreements(&targets, &ids);
        assert_eq!(out.len(), 1);
        let (message, data) = &out[0];
        assert!(
            message.contains("chromium found 30, firefox found 6; missing in firefox: c_000000000006, c_000000000007, c_000000000008, c_000000000009, c_000000000010 (and 19 more)."),
            "{message}"
        );
        assert!(!message.contains("missing in chromium"), "{message}");
        assert!(message.ends_with("Try a settle time."));
        assert_eq!(data["code"], "browsers_disagree");
        assert_eq!(data["viewport"]["width"], 1280);
        assert_eq!(data["counts"]["firefox"], 6);
        assert_eq!(data["missing"]["firefox"].as_array().unwrap().len(), 20);
        assert_eq!(data["missingTotal"]["firefox"], 24);
        assert!(data["missing"].get("chromium").is_none());
        // Same count, different elements: no warning here (the findings rule reports it).
        let swapped = [everything[..6].to_vec(), everything[1..7].to_vec()];
        let same = [summary("chromium", 6), summary("firefox", 6)];
        assert!(disagreements(&same, &swapped).is_empty());
    }

    #[test]
    fn an_exclusive_browser_is_held_by_one_target_at_a_time() {
        let exclusive = Exclusive::default();
        let never = AtomicBool::new(false);
        let inside = AtomicUsize::new(0);
        let most_at_once = AtomicUsize::new(0);
        thread::scope(|scope| {
            for _ in 0..6 {
                scope.spawn(|| {
                    let _held = exclusive.hold("safari", &never).unwrap();
                    most_at_once.fetch_max(inside.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                    thread::sleep(std::time::Duration::from_millis(5));
                    inside.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        assert_eq!(most_at_once.load(Ordering::SeqCst), 1);
        // A different browser is not held up by it.
        let _safari = exclusive.hold("safari", &never).unwrap();
        let _other = exclusive.hold("another", &never).unwrap();
    }

    #[test]
    fn a_waiter_for_an_exclusive_browser_notices_cancel_within_a_slice() {
        let exclusive = Arc::new(Exclusive::default());
        let never = AtomicBool::new(false);
        let cancel = Arc::new(AtomicBool::new(false));
        let held = exclusive.hold("safari", &never).unwrap();
        // The waiter runs on its own thread so that a `hold` which ignores `cancel` (the bug this
        // guards against) turns into a failed test after 5 s instead of a hung test run.
        let (done, outcome) = std::sync::mpsc::channel();
        let waiter = {
            let (exclusive, cancel) = (Arc::clone(&exclusive), Arc::clone(&cancel));
            thread::spawn(move || {
                let started = Instant::now();
                let gave_up = exclusive.hold("safari", &cancel).is_none();
                let _ = done.send((gave_up, started.elapsed()));
            })
        };
        thread::sleep(std::time::Duration::from_millis(100));
        cancel.store(true, Ordering::SeqCst);
        let result = outcome.recv_timeout(std::time::Duration::from_secs(5));
        let Ok((gave_up, waited)) = result else {
            // Let a waiter that does notice cancel end; the thread is leaked if it does not.
            drop(held);
            panic!("the waiter did not return within 5 s of cancel: Exclusive::hold must look at cancel every CANCEL_SLICE (L8)");
        };
        waiter.join().unwrap();
        assert!(gave_up, "a cancelled waiter must get None");
        assert!(waited < std::time::Duration::from_millis(900), "waited {waited:?}");
        // A cancelled waiter took nothing: once the holder is done the browser is free again.
        drop(held);
        assert!(exclusive.hold("safari", &never).is_some());
    }

    #[test]
    fn engine_error_display_text_is_unchanged() {
        // What the CLI and the MCP server print today.
        assert_eq!(EngineError::Cancelled.to_string(), "cancelled");
        assert_eq!(
            EngineError::invalid("a run needs at least one target and one viewport").to_string(),
            "a run needs at least one target and one viewport"
        );
        let driver = DriverError::new(ErrorKind::NavigationFailed, "net::ERR_NAME_NOT_RESOLVED");
        assert_eq!(
            EngineError::from(driver).to_string(),
            "net::ERR_NAME_NOT_RESOLVED (navigation_failed)"
        );
        let io = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access is denied");
        assert_eq!(EngineError::from(io).to_string(), "access is denied");
    }

    #[test]
    fn cancelled_is_distinguishable_from_failed() {
        let timeout = EngineError::from(DriverError::new(ErrorKind::Timeout, "slow"));
        let failed = EngineError::from(DriverError::new(ErrorKind::EvalFailed, "boom"));
        assert!(EngineError::Cancelled.is_cancelled() && !EngineError::Cancelled.is_timed_out());
        assert!(timeout.is_timed_out() && !timeout.is_cancelled());
        assert!(!failed.is_timed_out() && !failed.is_cancelled());
        assert!(!EngineError::invalid("cancelled").is_cancelled());
    }

    fn summary_with(status: RunStatus) -> TargetSummary {
        TargetSummary {
            dir: "firefox-1280x800".into(),
            target: BrowserTarget::named("firefox"),
            viewport: Viewport {
                width: 1280,
                height: 800,
            },
            browser: None,
            status,
            component_count: 0,
            instance_count: 0,
            warning_count: 0,
            duration_ms: 0,
            timings: None,
            error: None,
        }
    }

    #[test]
    fn roll_up_status_prefers_cancelled_then_failed_then_completed() {
        use RunStatus::{Cancelled, Completed, Failed};
        let rolled = |statuses: &[RunStatus]| {
            let targets: Vec<TargetSummary> = statuses.iter().map(|status| summary_with(*status)).collect();
            roll_up_status(&targets)
        };
        assert_eq!(rolled(&[Completed, Completed]), Completed);
        assert_eq!(rolled(&[Completed, Failed]), Failed);
        assert_eq!(rolled(&[Failed, Cancelled, Completed]), Cancelled);
        assert_eq!(rolled(&[Cancelled]), Cancelled);
        // Nothing to report on is not a failure.
        assert_eq!(rolled(&[]), Completed);
    }

    #[test]
    fn only_timeouts_are_retried_and_only_when_targets_ran_side_by_side() {
        let finished = |timed_out| Finished {
            ids: Vec::new(),
            summary: summary_with(RunStatus::Failed),
            timed_out,
        };
        let finished = |timed_out| Some(finished(timed_out));
        let results = [finished(false), finished(true), finished(false), finished(true), None];
        assert_eq!(timed_out_targets(2, &results), [1, 3]);
        // A target that had the machine to itself already got its fair chance.
        assert!(timed_out_targets(1, &results).is_empty());
        assert!(timed_out_targets(4, &[finished(false)]).is_empty());
    }

    #[test]
    fn the_plan_names_every_browser_at_every_viewport_and_caps_the_workers() {
        let mut params = request("extract", "https://shop.test", 2);
        params.viewports.push(Viewport {
            width: 390,
            height: 844,
        });
        params.parallel = Some(16);
        let (jobs, workers) = plan(&params);
        let names: Vec<&str> = jobs.iter().map(|job| job.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "firefox-1280x800",
                "firefox-390x844",
                "firefox-1280x800-2",
                "firefox-390x844-2"
            ]
        );
        assert_eq!(workers, 4);
        params.parallel = Some(2);
        assert_eq!(plan(&params).1, 2);
        params.parallel = Some(0);
        assert!((1..=4).contains(&plan(&params).1));
    }

    #[test]
    fn the_default_number_of_browsers_at_once_is_sensible() {
        assert!((1..=4).contains(&default_parallelism()));
    }

    #[test]
    fn target_folders_never_collide() {
        let mut used = HashSet::new();
        let viewport = Viewport {
            width: 390,
            height: 844,
        };
        let target = |browser: &str| BrowserTarget::named(browser);
        assert_eq!(unique_name(&mut used, &target("webkit"), &viewport), "webkit-390x844");
        assert_eq!(unique_name(&mut used, &target("webkit"), &viewport), "webkit-390x844-2");
        assert_eq!(unique_name(&mut used, &target("../evil"), &viewport), "___evil-390x844");
    }

    #[test]
    fn internal_error_text_redacts_addresses_before_truncating() {
        let text = internal_error_text("boom https://h/p?token=SECRET123 and more");
        assert!(!text.contains("SECRET123"), "{text}");
        assert!(text.starts_with("internal error: boom "), "{text}");
        // A secret that straddles the 200th character must not leak its first half either.
        let message = format!("{} https://h/p?token=SECRET123456789", "x".repeat(180));
        let text = internal_error_text(&message);
        assert!(!text.contains("SECRET") && !text.contains("SEC"), "{text}");
    }

    #[test]
    fn internal_error_text_is_bounded_to_200_chars_plus_prefix() {
        let text = internal_error_text(&"é".repeat(5000));
        assert_eq!(text.chars().count(), "internal error: ".len() + 200);
    }

    #[test]
    fn a_cancel_that_is_already_set_explains_any_error() {
        let cancel = AtomicBool::new(true);
        let died = EngineError::from(DriverError::new(
            ErrorKind::Transport,
            "driver exited while handling page.scroll",
        ));
        assert!(stopped_by_cancel(&died, &cancel, Duration::ZERO));
        let page = EngineError::from(DriverError::new(ErrorKind::NavigationFailed, "net::ERR_FAILED"));
        assert!(stopped_by_cancel(&page, &cancel, Duration::ZERO));
    }

    #[test]
    fn a_cancel_that_arrives_just_after_the_driver_died_still_counts() {
        let cancel = AtomicBool::new(false);
        let died = EngineError::from(DriverError::new(ErrorKind::Transport, "driver exited"));
        thread::scope(|scope| {
            scope.spawn(|| {
                thread::sleep(Duration::from_millis(80));
                cancel.store(true, Ordering::SeqCst);
            });
            assert!(stopped_by_cancel(&died, &cancel, Duration::from_millis(1500)));
        });
    }

    #[test]
    fn a_real_failure_without_cancel_stays_failed() {
        let cancel = AtomicBool::new(false);
        let died = EngineError::from(DriverError::new(ErrorKind::Transport, "driver exited"));
        assert!(!stopped_by_cancel(&died, &cancel, Duration::from_millis(30)));
        // Errors about the page are not waited for at all.
        let page = EngineError::from(DriverError::new(ErrorKind::NavigationFailed, "net::ERR_FAILED"));
        let started = Instant::now();
        assert!(!stopped_by_cancel(&page, &cancel, Duration::from_secs(5)));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(!stopped_by_cancel(
            &EngineError::invalid("bad"),
            &cancel,
            Duration::from_secs(5)
        ));
    }

    #[test]
    fn a_stopped_run_whose_targets_all_completed_is_completed() {
        let targets = [summary_with(RunStatus::Completed), summary_with(RunStatus::Completed)];
        assert_eq!(roll_up_status(&targets), RunStatus::Completed);
    }

    #[test]
    fn abort_is_closed_only_once_the_run_folder_is() {
        let abort = Abort::new();
        assert!(!abort.is_requested());
        let run = abort.register();
        abort.request();
        assert!(abort.is_requested());
        assert!(!abort.wait_closed(Duration::from_millis(30)));
        thread::scope(|scope| {
            scope.spawn(|| {
                thread::sleep(Duration::from_millis(30));
                run.release();
            });
            assert!(abort.wait_closed(Duration::from_secs(5)));
        });
    }

    #[test]
    fn abort_with_two_runs_is_closed_only_when_both_are() {
        let abort = Abort::new();
        assert!(abort.wait_quiet(Duration::ZERO), "no run, nothing to wait for");
        assert!(!abort.wait_closed(Duration::from_millis(10)), "no run closed yet");
        let first = abort.register();
        let second = abort.register();
        first.release();
        assert!(
            !abort.wait_closed(Duration::from_millis(30)),
            "the second is still open"
        );
        second.release();
        assert!(abort.wait_closed(Duration::from_millis(30)));
        first.release();
        assert!(abort.wait_closed(Duration::ZERO), "a second release changes nothing");
    }

    #[test]
    fn abort_is_idle_when_every_in_flight_guard_has_dropped() {
        let abort = Abort::new();
        assert!(abort.wait_idle(Duration::ZERO));
        let one = abort.enter();
        let two = abort.enter();
        assert!(!abort.wait_idle(Duration::from_millis(20)));
        drop(one);
        assert!(!abort.wait_idle(Duration::from_millis(20)));
        thread::scope(|scope| {
            scope.spawn(|| {
                thread::sleep(Duration::from_millis(30));
                drop(two);
            });
            assert!(abort.wait_idle(Duration::from_secs(5)));
        });
    }
}
