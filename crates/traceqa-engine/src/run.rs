//! One run: every requested browser at every requested viewport.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::thread;
use std::time::Instant;

use serde_json::json;
use traceqa_protocol::types::{BrowserTarget, Event, RunManifest, RunStartParams, RunStatus, TargetSummary, Viewport};
use traceqa_recorder::{new_run_id, now_rfc3339, RunWriter};

use crate::driver::{DriverCommand, DriverError, Drivers};
use crate::extract::{extract_target, Job, Progress, Sink};
use crate::url_safety::{check_navigable, redact};

pub(crate) const RUN_FOLDER_VERSION: u64 = traceqa_protocol::contracts::run_folder::VERSION;

/// Features this engine can run.
pub const FEATURES: [&str; 1] = ["extract"];

/// Drivers used when none are configured: from PATH, else the copies `traceqa install` made.
const DEFAULT_DRIVERS: [&str; 2] = ["traceqa-driver-playwright", "traceqa-driver-webdriver"];

/// Environment variable holding driver command lines separated by `;`.
pub const DRIVERS_ENV: &str = "TRACEQA_DRIVERS";

#[derive(Debug)]
pub struct EngineError {
    message: String,
    cancelled: bool,
    /// The browser or its driver did not answer in time.
    timed_out: bool,
}

impl EngineError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            cancelled: false,
            timed_out: false,
        }
    }

    pub(crate) fn cancelled() -> Self {
        Self {
            message: "cancelled".into(),
            cancelled: true,
            timed_out: false,
        }
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for EngineError {}

impl From<DriverError> for EngineError {
    fn from(error: DriverError) -> Self {
        Self {
            timed_out: error.kind == "timeout",
            ..Self::new(error.to_string())
        }
    }
}

impl From<std::io::Error> for EngineError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
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
}

impl Run {
    /// Checks the request and creates the run folder.
    pub fn prepare(params: RunStartParams) -> Result<Self, EngineError> {
        if !FEATURES.contains(&params.feature.as_str()) {
            return Err(EngineError::new(format!(
                "unknown feature {:?}. This engine runs: {}",
                params.feature,
                FEATURES.join(", ")
            )));
        }
        check_navigable(&params.url).map_err(EngineError::new)?;
        if params.targets.is_empty() || params.viewports.is_empty() {
            return Err(EngineError::new("a run needs at least one target and one viewport"));
        }
        let out_dir = PathBuf::from(params.out_dir.as_deref().unwrap_or("runs"));
        // An id that is already taken on disk (another process, same second) is simply drawn again.
        let mut attempts = 0;
        let writer = loop {
            match RunWriter::create(&out_dir, &new_run_id()) {
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && attempts < 20 => attempts += 1,
                other => break other?,
            }
        };
        Ok(Self { params, writer })
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
        let Self { params, writer } = self;
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

        // Every browser at every viewport, each with the folder name it will write to.
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

        let sink = Mutex::new(Sink { writer, listener });
        let progress = Progress { sink: &sink };
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
        };
        let next = AtomicUsize::new(0);
        let finished: Mutex<Vec<Option<Finished>>> = Mutex::new(jobs.iter().map(|_| None).collect());
        thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| {
                    // A driver process serves one browser at a time, so each worker has its own.
                    let mut drivers = (execution.drivers)();
                    while let Some((index, job)) = claim(&next, &jobs) {
                        let outcome = run_target(job, &shared, &mut drivers);
                        finished.lock().unwrap_or_else(|poisoned| poisoned.into_inner())[index] = Some(outcome);
                    }
                });
            }
        });
        let mut finished: Vec<Finished> = finished
            .into_inner()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .into_iter()
            .map(|outcome| outcome.expect("every job is claimed by a worker"))
            .collect();

        // A page that is heavy to render can time out only because several browsers were
        // rendering it at once. Such a target gets a second go with the machine to itself.
        if workers > 1 {
            for (job, outcome) in jobs.iter().zip(&mut finished).filter(|(_, outcome)| outcome.timed_out) {
                if execution.cancel.load(Ordering::Relaxed) {
                    break;
                }
                progress.emit(
                    "target.retried",
                    Some(&job.name),
                    format!(
                        "Trying {} again on its own: it timed out while other browsers were running",
                        job.name
                    ),
                    json!({}),
                );
                *outcome = run_target(job, &shared, &mut (execution.drivers)());
            }
        }
        // In the order they were asked for, whatever order they finished in.
        manifest.targets = finished.into_iter().map(|outcome| outcome.summary).collect();

        for message in disagreements(&manifest.targets) {
            progress.emit("warning", None, message, json!({ "code": "browsers_disagree" }));
        }

        let has = |status| manifest.targets.iter().any(|target| target.status == status);
        manifest.status = if has(RunStatus::Cancelled) {
            RunStatus::Cancelled
        } else if has(RunStatus::Failed) {
            RunStatus::Failed
        } else {
            RunStatus::Completed
        };
        manifest.finished_at = Some(now_rfc3339());
        progress.emit(
            "run.finished",
            None,
            format!("Run {}", manifest.status),
            json!({ "status": manifest.status }),
        );
        let writer = sink
            .into_inner()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .writer;
        save(&writer, &manifest);
        manifest
    }
}

/// How a run is carried out.
pub struct Execution<'a> {
    /// Makes the drivers one worker uses. It is called once per worker.
    pub drivers: &'a (dyn Fn() -> Drivers + Sync),
    /// Set this to stop the run. Targets that have not started are skipped, and the ones
    /// that are running stop at their next step.
    pub cancel: &'a AtomicBool,
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
}

/// How many browsers to run at once when the request does not say. Each one is a browser
/// process that renders a page, so half the cores is a safe share, and beyond four the gain
/// is small while the memory use is not.
fn default_parallelism() -> usize {
    let cores = thread::available_parallelism().map_or(1, |cores| cores.get());
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
                storage_state: shared.params.storage_state.as_ref(),
            };
            let (file, browser, timings) = extract_target(drivers, &extraction, progress)?;
            progress.with_writer(|writer| writer.write_components(name, &file))?;
            Ok((file, browser, timings))
        });
    summary.duration_ms = started.elapsed().as_millis() as u64;

    match outcome {
        Ok((file, browser, timings)) => {
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
        Err(error) if error.cancelled => {}
        Err(error) => {
            summary.status = RunStatus::Failed;
            summary.error = Some(error.to_string());
            progress.emit("error", Some(name), format!("{name} failed: {error}"), json!({}));
            return Finished {
                summary,
                timed_out: error.timed_out,
            };
        }
    }
    Finished {
        summary,
        timed_out: false,
    }
}

/// Browsers of which only one session may exist on the machine. A target that needs one
/// waits here until the target that has it is done.
#[derive(Default)]
pub(crate) struct Exclusive {
    busy: Mutex<HashSet<String>>,
    freed: Condvar,
}

impl Exclusive {
    /// Waits until `browser` is free and takes it until the returned value is dropped.
    pub(crate) fn hold<'a>(&'a self, browser: &str) -> Held<'a> {
        let mut busy = self.busy.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        while busy.contains(browser) {
            busy = self.freed.wait(busy).unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        busy.insert(browser.to_string());
        Held {
            owner: self,
            browser: browser.to_string(),
        }
    }
}

pub(crate) struct Held<'a> {
    owner: &'a Exclusive,
    browser: String,
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        self.owner
            .busy
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.browser);
        self.owner.freed.notify_all();
    }
}

fn save(writer: &RunWriter, manifest: &RunManifest) {
    if let Err(error) = writer.write_manifest(manifest) {
        eprintln!("traceqa: could not write run.json: {error}");
    }
}

/// One message per viewport at which the browsers that finished found different numbers of
/// elements. The same page should hold the same controls in every browser, so a difference
/// usually means one of them was read before the page had finished appearing.
fn disagreements(targets: &[TargetSummary]) -> Vec<String> {
    let mut by_viewport: Vec<((u64, u64), Vec<&TargetSummary>)> = Vec::new();
    for target in targets.iter().filter(|target| target.status == RunStatus::Completed) {
        let size = (target.viewport.width, target.viewport.height);
        match by_viewport.iter_mut().find(|(seen, _)| *seen == size) {
            Some((_, group)) => group.push(target),
            None => by_viewport.push((size, vec![target])),
        }
    }
    by_viewport
        .into_iter()
        .filter(|(_, group)| group.iter().any(|target| target.instance_count != group[0].instance_count))
        .map(|((width, height), group)| {
            let found: Vec<String> =
                group.iter().map(|target| format!("{} found {}", target.target.browser, target.instance_count)).collect();
            format!(
                "Browsers disagree at {width}x{height}: {}. The page may not have finished appearing when it was read. Try a settle time.",
                found.join(", ")
            )
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
        let messages = disagreements(&targets);
        assert_eq!(messages.len(), 1);
        assert!(
            messages[0].contains("390x800: chromium found 81, firefox found 54"),
            "{}",
            messages[0]
        );
    }

    #[test]
    fn an_exclusive_browser_is_held_by_one_target_at_a_time() {
        let exclusive = Exclusive::default();
        let inside = AtomicUsize::new(0);
        let most_at_once = AtomicUsize::new(0);
        thread::scope(|scope| {
            for _ in 0..6 {
                scope.spawn(|| {
                    let _held = exclusive.hold("safari");
                    most_at_once.fetch_max(inside.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                    thread::sleep(std::time::Duration::from_millis(5));
                    inside.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        assert_eq!(most_at_once.load(Ordering::SeqCst), 1);
        // A different browser is not held up by it.
        let _safari = exclusive.hold("safari");
        let _other = exclusive.hold("another");
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
}
