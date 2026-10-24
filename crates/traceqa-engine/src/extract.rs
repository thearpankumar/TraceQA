//! Component extraction for one browser at one viewport.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use traceqa_protocol::types::{
    BrowserInfo, BrowserTarget, CollectorResult, Component, ComponentsFile, Event, FrameInfo, PageEvalParams,
    PageGotoParams, PageScreenshotParams, PageScrollParams, Rect, ScrollInfo, ScrollTo, ScrollWait, SessionOpenParams,
    SessionOpenResult, StorageState, Timings, Viewport, WaitUntil, Warning,
};
use traceqa_recorder::{now_rfc3339, save_crops, CropRequest, PageImage, RunWriter};

use crate::driver::{Driver, Drivers, DEFAULT_TIMEOUT_MS};
use crate::group::{group, Located};
use crate::run::{Exclusive, RUN_FOLDER_VERSION};
use crate::url_safety::redact;
use crate::{fingerprint, EngineError, COLLECTOR};

/// How many screens the engine scrolls through before it stops reading a very long page.
const DEFAULT_SCROLL_STEPS: u64 = 50;

/// The run folder's writer and the live listener. Every target of a run shares them.
pub(crate) struct Sink<'a> {
    pub writer: RunWriter,
    pub listener: &'a mut (dyn FnMut(&Event) + Send),
}

/// Records events in the run folder and passes them to whoever is listening live. Targets
/// running side by side each hold a copy, and an event is written and delivered as one step,
/// so the listener sees events in the order they have in `events.jsonl`.
#[derive(Clone, Copy)]
pub(crate) struct Progress<'a, 'b> {
    pub sink: &'a Mutex<Sink<'b>>,
}

impl Progress<'_, '_> {
    pub fn emit(&self, kind: &str, target: Option<&str>, message: String, data: Value) {
        let mut guard = self.sink.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let sink = &mut *guard;
        match sink.writer.event(kind, target, message, data) {
            Ok(event) => (sink.listener)(&event),
            Err(error) => eprintln!("traceqa: could not write events.jsonl: {error}"),
        }
    }

    /// Uses the run folder's writer for something other than an event.
    pub fn with_writer<T>(&self, work: impl FnOnce(&RunWriter) -> T) -> T {
        work(&self.sink.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).writer)
    }
}

/// Where the time of one extraction went.
#[derive(Default)]
struct Clock {
    open: Duration,
    load: Duration,
    read: Duration,
    scroll: Duration,
    screenshot: Duration,
    crop: Duration,
    screens: u64,
}

impl Clock {
    fn timings(&self) -> Timings {
        let ms = |spent: Duration| spent.as_millis() as u64;
        Timings {
            open_ms: ms(self.open),
            load_ms: ms(self.load),
            read_ms: ms(self.read),
            scroll_ms: ms(self.scroll),
            screenshot_ms: ms(self.screenshot),
            crop_ms: ms(self.crop),
            screens: Some(self.screens),
        }
    }
}

/// Does `work` and adds the time it took to `spent`.
fn timed<T>(spent: &mut Duration, work: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let done = work();
    *spent += started.elapsed();
    done
}

/// Everything that identifies one extraction.
pub(crate) struct Job<'a> {
    pub run_id: &'a str,
    /// The real address: the only one the browser is sent to.
    pub url: &'a str,
    /// The address with secrets hidden: the one to write down or report.
    pub display_url: &'a str,
    pub target: &'a BrowserTarget,
    pub viewport: &'a Viewport,
    pub max_scroll_steps: Option<u64>,
    pub timeout_ms: Option<u64>,
    pub crops: bool,
    pub include_hidden: bool,
    /// Extra time to wait after the page has loaded, before reading it.
    pub settle_ms: u64,
    /// Name of the target's folder, which also labels its events.
    pub name: &'a str,
    pub dir: &'a Path,
    /// Shared by every target of the run, for browsers that allow one session at a time.
    pub exclusive: &'a Exclusive,
    /// Cookies and stored values every browser starts with, for pages behind a sign-in.
    pub storage_state: Option<&'a StorageState>,
    pub cancel: &'a AtomicBool,
}

fn warning(code: &str, message: String) -> Warning {
    Warning {
        code: code.to_string(),
        message,
    }
}

/// Opens a session, reads the page and always closes the session again.
pub(crate) fn extract_target(
    drivers: &mut Drivers,
    job: &Job,
    progress: Progress,
) -> Result<(ComponentsFile, BrowserInfo, Timings), EngineError> {
    let mut clock = Clock::default();
    let opening = Instant::now();
    let driver = drivers.for_browser(&job.target.browser)?;
    // Held until this function returns, so no other target opens the same browser meanwhile.
    let _only_session = driver
        .is_exclusive(&job.target.browser)
        .then(|| job.exclusive.hold(&job.target.browser));
    let session = driver.open_session(&SessionOpenParams {
        target: job.target.clone(),
        viewport: job.viewport.clone(),
        locale: None,
        storage_state: job.storage_state.cloned(),
    })?;
    clock.open = opening.elapsed();

    // A browser may not reach the size asked for. The page is then read, scrolled, cropped and
    // recorded at the size it really has, and a warning says so.
    let reached = session.viewport.clone().filter(|reached| reached != job.viewport);
    let at_reached = reached.as_ref().map(|viewport| Job { viewport, ..*job });
    let result = read_page(
        driver,
        &session,
        at_reached.as_ref().unwrap_or(job),
        progress,
        &mut clock,
    )
    .map(|mut file| {
        if let Some(reached) = &reached {
            let message = format!(
                "The browser could not give a {}x{} viewport, so the page was read at {}x{}.",
                job.viewport.width, job.viewport.height, reached.width, reached.height
            );
            progress.emit(
                "warning",
                Some(job.name),
                message.clone(),
                json!({ "code": "viewport_not_reached" }),
            );
            file.warnings.push(warning("viewport_not_reached", message));
        }
        file
    });
    // A driver that stopped answering cannot close anything. It is replaced before its next use.
    if !driver.is_lost() {
        if let Err(error) = driver.close_session(&session.session_id) {
            eprintln!("traceqa: could not close the {} session: {error}", job.target.browser);
        }
    }
    result.map(|file| (file, session.browser, clock.timings()))
}

fn check_cancelled(job: &Job) -> Result<(), EngineError> {
    if job.cancel.load(Ordering::Relaxed) {
        return Err(EngineError::cancelled());
    }
    Ok(())
}

/// What one pass of the collector over the page and its frames produced.
struct Reading {
    elements: Vec<Located>,
    title: String,
    scroll: ScrollInfo,
    warnings: Vec<Warning>,
    frames: usize,
}

/// Runs the collector in the main page and every frame the driver can reach.
fn read_frames(driver: &mut Driver, session: &SessionOpenResult, job: &Job) -> Result<Reading, EngineError> {
    let evaluated = driver.eval(&PageEvalParams {
        session_id: session.session_id.clone(),
        script: COLLECTOR.to_string(),
        arg: Some(json!({ "includeHidden": job.include_hidden })),
        all_frames: Some(session.capabilities.eval_all_frames),
        timeout_ms: Some(job.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS)),
    })?;

    let frames = evaluated.results.len();
    let mut elements = Vec::new();
    let mut warnings = Vec::new();
    let mut main: Option<(String, ScrollInfo)> = None;
    for result in evaluated.results {
        let frame = result.frame;
        let is_main = frame.id == "main";
        let collected = match result.value {
            Some(value) => serde_json::from_value::<CollectorResult>(value).map_err(|e| e.to_string()),
            None => Err(result.error.unwrap_or_else(|| "the driver gave no reason".to_string())),
        };
        match collected {
            Ok(collected) => {
                warnings.extend(collected.warnings);
                let frame_step = (!is_main).then(|| format!("frame \"{}\"", frame_label(&frame)));
                for element in collected.elements {
                    let page_box = if is_main {
                        Some(element.box_.clone())
                    } else {
                        frame.box_.as_ref().map(|f| offset(&element.box_, f))
                    };
                    elements.push(Located {
                        element,
                        frame_id: frame.id.clone(),
                        frame_step: frame_step.clone(),
                        page_box,
                        name_changes: false,
                    });
                }
                if is_main {
                    main = Some((collected.title, collected.scroll));
                }
            }
            Err(reason) if is_main => return Err(EngineError::new(format!("could not read the page: {reason}"))),
            Err(reason) => {
                let code = if frame.cross_origin {
                    "cross_origin_frame_skipped"
                } else {
                    "frame_eval_failed"
                };
                warnings.push(warning(
                    code,
                    format!("Could not read the frame at {}: {reason}", redact(&frame.url)),
                ));
            }
        }
    }
    let (title, scroll) = main.ok_or_else(|| EngineError::new("the driver returned nothing for the main frame"))?;
    Ok(Reading {
        elements,
        title,
        scroll,
        warnings,
        frames,
    })
}

/// One screenful of the page as it looked while the engine scrolled through it.
struct Screen {
    image: PathBuf,
    /// Page y coordinate of the top edge of the screen.
    top: f64,
    height: f64,
    /// Where each visible element was at that moment, in page coordinates. Elements in pinned
    /// or animated sections move as the page scrolls, so this is recorded per screen.
    shown: HashMap<ElementKey, Rect>,
}

/// Identifies one element across readings: its frame and its structural selector.
type ElementKey = (String, String);

fn element_key(frame_id: &str, selectors: &[String]) -> ElementKey {
    (frame_id.to_string(), selectors.last().cloned().unwrap_or_default())
}

impl Screen {
    /// How much of `area` this screen shows, from 0 to 1.
    fn coverage(&self, area: &Rect, width: f64) -> f64 {
        let across = (area.x + area.w).min(width) - area.x.max(0.0);
        let down = (area.y + area.h).min(self.top + self.height) - area.y.max(self.top);
        if area.w <= 0.0 || area.h <= 0.0 || across <= 0.0 || down <= 0.0 {
            return 0.0;
        }
        (across * down) / (area.w * area.h)
    }
}

/// True when the page is scrolled as far down as it goes.
fn at_end(scroll: &ScrollInfo) -> bool {
    scroll.y + scroll.viewport_height >= scroll.height - 1.0
}

/// Address words that mean a sign-in page.
const SIGN_IN_WORDS: [&str; 6] = ["login", "log-in", "signin", "sign-in", "sign_in", "auth"];

/// A run given cookies and stored values that still lands on what looks like a sign-in page
/// was not let in: the session has expired, or the state belongs to another site. Without this
/// the run would quietly read the sign-in page in place of the page asked for.
fn sign_in_warning(reading: &Reading, final_url: &str) -> Option<Warning> {
    let path = final_url
        .split_once("://")
        .map_or(final_url, |(_, rest)| rest)
        .to_lowercase();
    let path = path.split_once('/').map_or("", |(_, path)| path);
    let address = SIGN_IN_WORDS.iter().any(|word| path.contains(word));
    let password = reading.elements.iter().any(|located| {
        located
            .element
            .attrs
            .get("type")
            .is_some_and(|kind| kind.eq_ignore_ascii_case("password"))
    });
    let sign = match (address, password) {
        (true, _) => "its address",
        (false, true) => "its password field",
        (false, false) => return None,
    };
    Some(warning(
        "session_not_accepted",
        format!(
            "The browser started with the stored session, but by {sign} the page is a sign-in page, so the session may have expired. Sign in again to make a new state file."
        ),
    ))
}

/// The least time between the two looks at the page that tell which names change. A countdown
/// ticks every second, so a second is enough to see it.
const NAME_WATCH: Duration = Duration::from_millis(1000);

/// Role, name and position of every element, by element.
type Names = HashMap<ElementKey, (String, String, Option<(f64, f64)>)>;

fn names_of(reading: &Reading) -> Names {
    reading
        .elements
        .iter()
        .map(|located| {
            let at = located.page_box.as_ref().map(|b| (b.x, b.y));
            let key = element_key(&located.frame_id, &located.element.selectors);
            (key, (located.element.role.clone(), located.element.name.clone(), at))
        })
        .collect()
}

/// Marks the elements whose name is not the one they had before the walk. Only the same
/// element counts: same place in the page, same role, same position. Content that loaded on
/// the way and moved others down is new, not renamed.
fn mark_changed_names(reading: &mut Reading, before: &Names) {
    for located in &mut reading.elements {
        let key = element_key(&located.frame_id, &located.element.selectors);
        let Some((role, name, at)) = before.get(&key) else {
            continue;
        };
        let now = located.page_box.as_ref().map(|b| (b.x, b.y));
        let same_place = match (at, now) {
            (Some(a), Some(b)) => (a.0 - b.0).abs() <= 2.0 && (a.1 - b.1).abs() <= 2.0,
            _ => false,
        };
        if same_place && *role == located.element.role && *name != located.element.name {
            located.name_changes = true;
        }
    }
}

/// Lists what can be clicked or typed into, as tag and text, quickly and without the
/// collector's detail. The page is not changed.
const LIST_CONTROLS: &str = "(function () {
  var found = document.querySelectorAll('a[href], area[href], button, input, select, textarea, summary, ' +
    '[role], [tabindex], [onclick], [contenteditable]');
  var keys = [];
  for (var i = 0; i < found.length && i < 5000; i++) {
    keys.push(found[i].localName + '|' + (found[i].textContent || found[i].value || '').trim().slice(0, 80));
  }
  return keys;
})";

/// Watches which controls the page holds while it is scrolled. A long table or feed that keeps
/// only the rows near the screen holds about as many at any moment, but not the same ones:
/// rows it showed earlier are gone, and so they are gone when the page is read.
#[derive(Default)]
struct Removals {
    seen: std::collections::HashSet<String>,
    most: u64,
    dropped: u64,
}

impl Removals {
    fn saw(&mut self, keys: Vec<String>) {
        let present: std::collections::HashSet<String> = keys.into_iter().collect();
        self.most = self.most.max(present.len() as u64);
        self.seen.extend(present.iter().cloned());
        self.dropped = self.dropped.max(self.seen.difference(&present).count() as u64);
    }

    /// A few controls come and go on any page, a menu closing for one. Only a real drop counts.
    fn warning(&self) -> Option<Warning> {
        (self.dropped >= 5 && self.dropped * 10 >= self.most).then(|| {
            warning(
                "elements_removed_while_scrolling",
                format!(
                    "The page removed {} controls it had shown while it was scrolled, as long lists that keep only the rows near the screen do. Rows that had left the screen are not listed.",
                    self.dropped
                ),
            )
        })
    }
}

fn list_controls(driver: &mut Driver, session: &SessionOpenResult, clock: &mut Clock) -> Option<Vec<String>> {
    let evaluated = timed(&mut clock.read, || {
        driver.eval(&PageEvalParams {
            session_id: session.session_id.clone(),
            script: LIST_CONTROLS.to_string(),
            arg: None,
            all_frames: Some(false),
            timeout_ms: Some(5_000),
        })
    });
    let value = evaluated.ok()?.results.into_iter().next()?.value?;
    serde_json::from_value(value).ok()
}

/// What going through the page produced.
struct Walk {
    /// The reading taken at the last position, when the walk took readings.
    reading: Option<Reading>,
    screens: Vec<Screen>,
    /// How many times the page was scrolled.
    steps: u64,
    /// Whether the bottom was reached. `None` when the walk could not tell.
    reached_end: Option<bool>,
    removals: Removals,
}

/// How long to wait at the bottom of a page for a part that is fetched only once the bottom is
/// reached, as feeds do. A page that fits on one screen is never waited for.
const PATIENCE_AT_END: Duration = Duration::from_millis(1000);

/// At the bottom of a page: waits for more to arrive, and says whether it did.
fn more_at_end(
    driver: &mut Driver,
    session: &SessionOpenResult,
    job: &Job,
    clock: &mut Clock,
    height: f64,
) -> Result<bool, EngineError> {
    check_cancelled(job)?;
    let scrolled = timed(&mut clock.scroll, || {
        thread::sleep(PATIENCE_AT_END);
        driver.scroll(&PageScrollParams {
            session_id: session.session_id.clone(),
            to: Some(ScrollTo::Bottom),
            by_y: None,
            wait_for: Some(ScrollWait::Height),
        })
    })?;
    Ok(scrolled.height > height + 1.0)
}

/// Goes through the page one screen at a time, the way a person scrolls, reading it and
/// taking a picture at each screen. That loads lazy content, lets reveal animations play,
/// and gives a picture of every part as it really looked.
fn walk_with_pictures(
    driver: &mut Driver,
    session: &SessionOpenResult,
    job: &Job,
    clock: &mut Clock,
    screens_dir: &Path,
    max_steps: u64,
    screenshot_warning: &mut Option<Warning>,
) -> Result<Walk, EngineError> {
    fs::create_dir_all(screens_dir)?;
    let mut screens: Vec<Screen> = Vec::new();
    let mut steps = 0;
    let mut removals = Removals::default();
    loop {
        check_cancelled(job)?;
        let reading = timed(&mut clock.read, || read_frames(driver, session, job))?;
        removals.saw(
            reading
                .elements
                .iter()
                .map(|located| {
                    format!(
                        "{}|{}",
                        located.element.tag,
                        located.element.text.as_deref().unwrap_or("")
                    )
                })
                .collect(),
        );
        let scroll = &reading.scroll;
        let image = screens_dir.join(format!("{steps}.png"));
        let captured = timed(&mut clock.screenshot, || {
            screenshot(driver, &session.session_id, &image, false, screenshot_warning)
        })?;
        if captured {
            let shown = reading
                .elements
                .iter()
                .filter(|located| located.element.state.visible)
                .filter_map(|located| {
                    let key = element_key(&located.frame_id, &located.element.selectors);
                    Some((key, located.page_box.clone()?))
                })
                .collect();
            screens.push(Screen {
                image,
                top: scroll.y,
                height: scroll.viewport_height,
                shown,
            });
        }

        let reached_end = at_end(scroll);
        // The bottom of a page that scrolls may only be the bottom for now.
        if reached_end
            && scroll.y > 0.0
            && steps < max_steps
            && more_at_end(driver, session, job, clock, scroll.height)?
        {
            steps += 1;
            continue;
        }
        if reached_end || steps >= max_steps {
            return Ok(Walk {
                reading: Some(reading),
                screens,
                steps,
                reached_end: Some(reached_end),
                removals,
            });
        }
        // A little less than a full screen, so nothing falls into the gap between two screens.
        let before = scroll.y;
        let scrolled = timed(&mut clock.scroll, || {
            driver.scroll(&PageScrollParams {
                session_id: session.session_id.clone(),
                to: None,
                by_y: Some(scroll.viewport_height * 0.9),
                wait_for: Some(ScrollWait::Animations),
            })
        })?;
        steps += 1;
        if scrolled.y <= before {
            // The page did not move: it scrolls inside a container, or not at all.
            return Ok(Walk {
                reading: Some(reading),
                screens,
                steps,
                reached_end: Some(true),
                removals,
            });
        }
    }
}

/// Goes through the page without looking at it: only scrolls, so that content which loads
/// on scroll gets loaded. With no picture to take there is nothing to read on the way and no
/// animation worth waiting for, which makes this several times quicker.
fn walk_quickly(
    driver: &mut Driver,
    session: &SessionOpenResult,
    job: &Job,
    clock: &mut Clock,
    max_steps: u64,
) -> Result<Walk, EngineError> {
    let mut walk = Walk {
        reading: None,
        screens: Vec::new(),
        steps: 0,
        reached_end: None,
        removals: Removals::default(),
    };
    if let Some(keys) = list_controls(driver, session, clock) {
        walk.removals.saw(keys);
    }
    let mut y = 0.0;
    while walk.steps < max_steps {
        check_cancelled(job)?;
        let scrolled = timed(&mut clock.scroll, || {
            driver.scroll(&PageScrollParams {
                session_id: session.session_id.clone(),
                to: None,
                by_y: Some(job.viewport.height as f64),
                wait_for: Some(ScrollWait::Height),
            })
        })?;
        walk.steps += 1;
        if let Some(keys) = list_controls(driver, session, clock) {
            walk.removals.saw(keys);
        }
        if scrolled.at_end || scrolled.y <= y {
            // The bottom of a page that scrolls may only be the bottom for now.
            if scrolled.y > 0.0 && walk.steps < max_steps && more_at_end(driver, session, job, clock, scrolled.height)?
            {
                walk.steps += 1;
                y = scrolled.y;
                continue;
            }
            walk.reached_end = Some(true);
            return Ok(walk);
        }
        y = scrolled.y;
    }
    // Out of steps. Whether that was the bottom is settled by the reading taken afterwards.
    Ok(walk)
}

fn read_page(
    driver: &mut Driver,
    session: &SessionOpenResult,
    job: &Job,
    progress: Progress,
    clock: &mut Clock,
) -> Result<ComponentsFile, EngineError> {
    let session_id = &session.session_id;
    let loading = Instant::now();
    let page = driver.goto(&PageGotoParams {
        session_id: session_id.clone(),
        url: job.url.to_string(),
        wait_until: Some(WaitUntil::Settled),
        timeout_ms: Some(job.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS)),
    })?;
    // The browser may have been redirected to an address that carries a token: write it down hidden.
    let shown_final_url = redact(&page.final_url);
    progress.emit(
        "page.opened",
        Some(job.name),
        format!(
            "Opened {} in {} {}",
            shown_final_url, session.browser.name, session.browser.version
        ),
        json!({ "finalUrl": shown_final_url, "status": page.status, "title": page.title }),
    );
    // The page is read either way: an error page or a page still loading can be the thing under test.
    let mut page_warnings = Vec::new();
    if let Some(status) = page.status.filter(|status| *status >= 400) {
        page_warnings.push(warning(
            "http_error_status",
            format!("The page answered with HTTP {status}, so this may be an error page."),
        ));
    }
    if let Some(closed) = page.closed_shadow_roots.filter(|closed| *closed > 0) {
        page_warnings.push(warning(
            "closed_shadow_roots",
            format!(
                "The page has {closed} closed shadow root{}. No script can read inside one, so what it holds is not listed.",
                if closed == 1 { "" } else { "s" }
            ),
        ));
    }
    if page.load_complete == Some(false) {
        page_warnings.push(warning(
            "page_load_incomplete",
            "The page's content arrived, but it had not finished loading when time ran out, so it was read as it stood."
                .to_string(),
        ));
    }

    // A timed intro or splash screen gives no sign of itself, so the caller says how long to wait.
    let mut waited = 0;
    while waited < job.settle_ms {
        check_cancelled(job)?;
        let pause = (job.settle_ms - waited).min(200);
        thread::sleep(Duration::from_millis(pause));
        waited += pause;
    }
    clock.load = loading.elapsed();

    let screens_dir = job.dir.join("screens");
    let max_steps = job.max_scroll_steps.unwrap_or(DEFAULT_SCROLL_STEPS);
    let mut screenshot_warning = None;
    // What every element is called before the walk, to see which names change by the end.
    let before = timed(&mut clock.read, || read_frames(driver, session, job)).map(|reading| names_of(&reading))?;
    let watched_since = Instant::now();

    let walk = if job.crops {
        walk_with_pictures(
            driver,
            session,
            job,
            clock,
            &screens_dir,
            max_steps,
            &mut screenshot_warning,
        )?
    } else {
        walk_quickly(driver, session, job, clock, max_steps)?
    };
    let Walk {
        reading,
        screens,
        steps,
        reached_end,
        removals,
    } = walk;
    clock.screens = steps + 1;

    // The list of elements is read with the page at the top, so positions do not depend on
    // where scrolling stopped, and long enough after the first look for a name that ticks,
    // as a countdown's does, to have changed.
    drop(reading);
    if steps > 0 {
        let wait_for = if job.crops {
            ScrollWait::Animations
        } else {
            ScrollWait::Height
        };
        timed(&mut clock.scroll, || {
            driver.scroll(&PageScrollParams {
                session_id: session_id.clone(),
                to: Some(ScrollTo::Top),
                by_y: None,
                wait_for: Some(wait_for),
            })
        })?;
    }
    if let Some(left) = NAME_WATCH.checked_sub(watched_since.elapsed()) {
        check_cancelled(job)?;
        thread::sleep(left);
    }
    let mut reading = timed(&mut clock.read, || read_frames(driver, session, job))?;
    mark_changed_names(&mut reading, &before);
    if job.storage_state.is_some() {
        page_warnings.extend(sign_in_warning(&reading, &page.final_url));
    }
    let reached_end = reached_end.unwrap_or_else(|| steps == 0 && at_end(&reading.scroll));
    if steps > 0 {
        progress.emit(
            "page.scrolled",
            Some(job.name),
            format!("Scrolled through the page in {} screens", steps + 1),
            json!({ "screens": steps + 1, "height": reading.scroll.height }),
        );
    }

    let Reading {
        elements,
        title,
        mut warnings,
        frames,
        ..
    } = reading;
    if !reached_end {
        warnings.push(warning(
            "scroll_limit_reached",
            format!("The page is longer than {max_steps} screens. Content further down was not read."),
        ));
    }
    warnings.extend(removals.warning());
    progress.emit(
        "elements.collected",
        Some(job.name),
        format!("Found {} elements in {frames} frames", elements.len()),
        json!({ "elements": elements.len(), "frames": frames }),
    );

    let mut components = group(elements);
    warnings.extend(covered_warning(&components));
    let instance_count: usize = components.iter().map(|component| component.instances.len()).sum();
    progress.emit(
        "components.grouped",
        Some(job.name),
        format!("Grouped {instance_count} elements into {} components", components.len()),
        json!({ "components": components.len(), "instances": instance_count }),
    );

    if job.crops {
        check_cancelled(job)?;
        let saved = timed(&mut clock.crop, || {
            save_component_crops(&mut components, &screens, job.viewport.width as f64, job.dir)
        })?;
        progress.emit(
            "crop.saved",
            Some(job.name),
            format!("Saved {saved} crops"),
            json!({ "crops": saved }),
        );

        // page.png is an overview: the whole page when the browser can capture that, else the first screen.
        let page_png = job.dir.join("page.png");
        let whole_page = session.capabilities.full_page_screenshot
            && timed(&mut clock.screenshot, || {
                screenshot(driver, session_id, &page_png, true, &mut screenshot_warning)
            })?;
        if !whole_page {
            if let Some(first) = screens.first() {
                fs::copy(&first.image, &page_png)?;
            }
        }
        fs::remove_dir_all(&screens_dir)?;
    }

    warnings.extend(page_warnings);
    warnings.extend(screenshot_warning);
    for item in &warnings {
        progress.emit(
            "warning",
            Some(job.name),
            item.message.clone(),
            json!({ "code": item.code }),
        );
    }

    Ok(ComponentsFile {
        format_version: RUN_FOLDER_VERSION,
        fingerprint_version: fingerprint::VERSION,
        run_id: job.run_id.to_string(),
        url: job.display_url.to_string(),
        final_url: shown_final_url,
        title,
        browser: session.browser.clone(),
        viewport: job.viewport.clone(),
        captured_at: now_rfc3339(),
        components,
        warnings,
    })
}

/// One warning for every component a click would not reach, because something lies on top of it:
/// a cookie banner or a modal's backdrop often covers a whole page at once.
fn covered_warning(components: &[Component]) -> Option<Warning> {
    let covered: Vec<(&Component, &str)> = components
        .iter()
        .filter_map(|component| {
            let instance = component.instances.iter().find(|i| i.state.covered == Some(true))?;
            Some((
                component,
                instance.state.covered_by.as_deref().unwrap_or("another element"),
            ))
        })
        .collect();
    let (first, by) = covered.first()?;
    let name = if first.name.is_empty() {
        "(no name)"
    } else {
        &first.name
    };
    let more = match covered.len() {
        1 => String::new(),
        n => format!(" and {} more", n - 1),
    };
    Some(warning(
        "covered_elements",
        format!(
            "{} {name:?} is under {by}{more}, so a click would not reach {}.",
            first.role,
            if covered.len() == 1 { "it" } else { "them" }
        ),
    ))
}

/// Takes a screenshot and says whether it worked. Pictures are evidence, not data, so a
/// screenshot the browser could not take is noted once and the extraction goes on. A driver
/// that has stopped answering is a different matter and ends the extraction.
fn screenshot(
    driver: &mut Driver,
    session_id: &str,
    path: &Path,
    full_page: bool,
    failure: &mut Option<Warning>,
) -> Result<bool, EngineError> {
    let taken = driver.screenshot(&PageScreenshotParams {
        session_id: session_id.to_string(),
        path: path.to_string_lossy().into_owned(),
        full_page: Some(full_page),
        clip: None,
    });
    match taken {
        Ok(_) => Ok(true),
        Err(error) if driver.is_lost() => Err(error.into()),
        Err(error) => {
            failure.get_or_insert_with(|| {
                warning(
                    "screenshot_failed",
                    format!(
                        "A screenshot could not be taken, so some crops are missing: {}",
                        error.message
                    ),
                )
            });
            Ok(false)
        }
    }
}

/// Saves one crop per component and returns how many were saved.
///
/// The crop comes from the screen that showed the most of the component's first instance that
/// was ever on screen. A component that never appeared while scrolling gets none.
fn save_component_crops(
    components: &mut [Component],
    screens: &[Screen],
    viewport_width: f64,
    dir: &Path,
) -> Result<usize, EngineError> {
    // For each screen, the crops to cut from it and the component each belongs to.
    let mut wanted: Vec<Vec<(usize, CropRequest)>> = screens.iter().map(|_| Vec::new()).collect();
    for (index, component) in components.iter().enumerate() {
        let best = component.instances.iter().find_map(|instance| {
            let key = element_key(&instance.frame_id, &instance.selectors);
            screens
                .iter()
                .enumerate()
                .filter_map(|(number, screen)| {
                    let area = screen.shown.get(&key)?;
                    let coverage = screen.coverage(area, viewport_width);
                    (coverage > 0.0).then(|| (number, area.clone(), coverage))
                })
                // The earliest of the screens that show the most of it.
                .reduce(|best, next| if next.2 > best.2 { next } else { best })
        });
        if let Some((number, area, _)) = best {
            wanted[number].push((
                index,
                CropRequest {
                    file_name: format!("{}.png", component.id),
                    area,
                },
            ));
        }
    }

    let mut saved = 0;
    for (screen, wanted) in screens.iter().zip(wanted) {
        let (owners, requests): (Vec<usize>, Vec<CropRequest>) = wanted.into_iter().unzip();
        let image = PageImage {
            path: &screen.image,
            top: screen.top,
            css_height: screen.height,
        };
        for ((index, request), written) in
            owners
                .into_iter()
                .zip(&requests)
                .zip(save_crops(&image, &requests, &dir.join("crops"))?)
        {
            if written {
                components[index].crop = Some(format!("crops/{}", request.file_name));
                saved += 1;
            }
        }
    }
    Ok(saved)
}

/// Moves a box from a frame's own coordinates to the top-level page.
fn offset(inner: &Rect, frame: &Rect) -> Rect {
    Rect {
        x: frame.x + inner.x,
        y: frame.y + inner.y,
        w: inner.w,
        h: inner.h,
    }
}

/// What a frame is called in a component path: its name, or the path of its URL.
fn frame_label(frame: &FrameInfo) -> &str {
    if let Some(name) = frame.name.as_deref().filter(|name| !name.is_empty()) {
        return name;
    }
    let after_scheme = frame.url.split_once("://").map_or(frame.url.as_str(), |(_, rest)| rest);
    let path = after_scheme.find('/').map_or("/", |start| &after_scheme[start..]);
    path.split(['?', '#']).next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shop example's component, under a cookie banner or not, renamed.
    fn component(name: &str, covered: Option<bool>) -> Component {
        let file: ComponentsFile =
            serde_json::from_str(include_str!("../../../spec/examples/ComponentsFile.shop.json")).unwrap();
        let mut component = file.components.into_iter().next().unwrap();
        component.name = name.to_string();
        for instance in &mut component.instances {
            instance.state.covered = covered;
            instance.state.covered_by = covered.filter(|c| *c).map(|_| "#cookie-banner".to_string());
        }
        component
    }

    #[test]
    fn covered_components_get_one_warning_that_names_the_first() {
        assert!(covered_warning(&[component("Buy", Some(false)), component("Help", None)]).is_none());

        let one = covered_warning(&[component("Buy", Some(false)), component("Accept", Some(true))]).unwrap();
        assert_eq!(one.code, "covered_elements");
        assert_eq!(
            one.message,
            "button \"Accept\" is under #cookie-banner, so a click would not reach it."
        );

        let several = covered_warning(&[component("Accept", Some(true)), component("Reject", Some(true))]).unwrap();
        assert!(several
            .message
            .ends_with("and 1 more, so a click would not reach them."));
    }

    fn frame(name: Option<&str>, url: &str) -> FrameInfo {
        FrameInfo {
            id: "f1".into(),
            parent_id: None,
            url: url.into(),
            name: name.map(Into::into),
            cross_origin: false,
            box_: None,
        }
    }

    #[test]
    fn frames_are_labelled_by_name_then_by_url_path() {
        assert_eq!(
            frame_label(&frame(Some("pay"), "https://pay.example.com/widget?id=7")),
            "pay"
        );
        assert_eq!(
            frame_label(&frame(None, "https://pay.example.com:8443/widget/v2?id=7#top")),
            "/widget/v2"
        );
        assert_eq!(frame_label(&frame(Some(""), "https://pay.example.com")), "/");
        assert_eq!(frame_label(&frame(None, "about:blank")), "/");
    }
}
