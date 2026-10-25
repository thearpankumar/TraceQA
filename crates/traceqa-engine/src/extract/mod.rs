//! Component extraction for one browser at one viewport.
//!
//! `read_page` is a short pipeline of named steps. Each step lives where its concern does:
//! `frames` reads the page, `walk` scrolls through it, `heuristics` judges what was read
//! (plain data in, warnings out), `crops` takes the pictures and `progress` records events
//! and time.

pub mod axe;
mod crops;
mod frames;
mod heuristics;
mod progress;
mod walk;

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;
use traceqa_protocol::types::{
    BrowserInfo, BrowserTarget, Component, ComponentsFile, PageGotoParams, PageScrollParams, ScrollTo, ScrollWait,
    SessionOpenParams, SessionOpenResult, StorageState, Timings, Viewport, WaitUntil, Warning,
};
use traceqa_recorder::now_rfc3339;

use self::crops::{save_component_crops, screenshot};
use self::frames::{read_frames, Reading};
use self::heuristics::{
    covered_warning, landing_warnings, mark_changed_names, names_of, requests_failed_warning, stored_session_warnings,
    Removals, NAME_WATCH,
};
use self::progress::{timed, Clock};
pub(crate) use self::progress::{Progress, Sink};
use self::walk::{at_end, walk_quickly, walk_with_pictures, Screen, Walk};
use crate::driver::{Driver, DriverError, Drivers, ErrorKind, DEFAULT_TIMEOUT_MS};
use crate::group::group;
use crate::run::{Exclusive, RUN_FOLDER_VERSION};
use crate::url_policy::UrlPolicy;
use crate::url_safety::redact;
use crate::{fingerprint, EngineError};

/// How many screens the engine scrolls through before it stops reading a very long page.
const DEFAULT_SCROLL_STEPS: u64 = 50;

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
    /// Operator rules for where the page may lead, checked after the page has loaded.
    pub policy: Option<&'a UrlPolicy>,
    /// axe-core's source when the run asked for it; the page is then also checked with axe.
    pub axe: Option<&'a axe::AxeScript>,
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
    let _only_session = if driver.is_exclusive(&job.target.browser) {
        Some(
            job.exclusive
                .hold(&job.target.browser, job.cancel)
                .ok_or(EngineError::Cancelled)?,
        )
    } else {
        None
    };
    let session = driver.until(job.cancel).open_session(&SessionOpenParams {
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
        return Err(EngineError::Cancelled);
    }
    Ok(())
}

/// Where the browser ended up after the address was opened.
struct Landed {
    /// The final address as the browser reported it. Only for reading; never written down.
    final_url: String,
    /// The final address with secrets hidden: the one to write down or report.
    shown_final_url: String,
    /// What the way in already says about the page.
    warnings: Vec<Warning>,
}

/// The page, read and described: everything `read_page` needs to assemble the result.
struct Survey {
    /// The list of elements, read with the page at the top.
    reading: Reading,
    screens: Vec<Screen>,
    /// How many times the page was scrolled.
    steps: u64,
    /// Whether the bottom of the page was reached.
    reached_end: bool,
    removals: Removals,
}

fn read_page(
    driver: &mut Driver,
    session: &SessionOpenResult,
    job: &Job,
    progress: Progress,
    clock: &mut Clock,
) -> Result<ComponentsFile, EngineError> {
    let loading = Instant::now();
    let mut landed = open_address(driver, session, job, progress)?;
    settle(job)?;
    clock.load = loading.elapsed();

    let screens_dir = job.dir.join("screens");
    let max_steps = job.max_scroll_steps.unwrap_or(DEFAULT_SCROLL_STEPS);
    let mut screenshot_warning = None;
    let Survey {
        reading,
        screens,
        steps,
        reached_end,
        removals,
    } = survey(
        driver,
        session,
        job,
        clock,
        &screens_dir,
        max_steps,
        &mut screenshot_warning,
    )?;
    if job.storage_state.is_some() {
        landed
            .warnings
            .extend(stored_session_warnings(&reading, &landed.final_url, job.crops));
    }
    if steps > 0 {
        progress.emit(
            "page.scrolled",
            Some(job.name),
            format!("Scrolled through the page in {} screens", steps + 1),
            json!({ "screens": steps + 1, "height": reading.scroll.height }),
        );
    }

    let failed_requests = requests_failed_warning(&reading);
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
    warnings.extend(failed_requests);
    progress.emit(
        "elements.collected",
        Some(job.name),
        format!("Found {} elements in {frames} frames", elements.len()),
        json!({ "elements": elements.len(), "frames": frames }),
    );

    let mut components = group(elements);
    warnings.extend(covered_warning(&components));
    axe::run(driver, session, job, &components, &mut warnings);
    let instance_count: usize = components.iter().map(|component| component.instances.len()).sum();
    progress.emit(
        "components.grouped",
        Some(job.name),
        format!("Grouped {instance_count} elements into {} components", components.len()),
        json!({ "components": components.len(), "instances": instance_count }),
    );

    if job.crops {
        save_pictures(
            driver,
            session,
            job,
            progress,
            clock,
            &mut components,
            &screens,
            &mut screenshot_warning,
        )?;
    }

    warnings.extend(landed.warnings);
    warnings.extend(screenshot_warning);
    announce_warnings(progress, job, &warnings);

    Ok(ComponentsFile {
        format_version: RUN_FOLDER_VERSION,
        fingerprint_version: fingerprint::VERSION,
        run_id: job.run_id.to_string(),
        url: job.display_url.to_string(),
        final_url: landed.shown_final_url,
        title,
        browser: session.browser.clone(),
        viewport: job.viewport.clone(),
        captured_at: now_rfc3339(),
        components,
        warnings,
    })
}

/// Goes to the address, refuses a place the operator's rules forbid, and says where it landed.
fn open_address(
    driver: &mut Driver,
    session: &SessionOpenResult,
    job: &Job,
    progress: Progress,
) -> Result<Landed, EngineError> {
    let page = driver.until(job.cancel).goto(&PageGotoParams {
        session_id: session.session_id.clone(),
        url: job.url.to_string(),
        wait_until: Some(WaitUntil::Settled),
        timeout_ms: Some(job.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS)),
    })?;
    // Redirects are only visible after the fact, so a refused place fails the target before
    // anything on the page is read or written down.
    if let Some(blocked) = job.policy.and_then(|policy| policy.check_landing(&page).err()) {
        return Err(DriverError::new(
            ErrorKind::BlockedUrl,
            format!(
                "the page led to a place this server does not allow: {blocked}. The request had already been made; nothing from the page was read"
            ),
        )
        .into());
    }
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
    Ok(Landed {
        warnings: landing_warnings(&page),
        final_url: page.final_url,
        shown_final_url,
    })
}

/// A timed intro or splash screen gives no sign of itself, so the caller says how long to wait.
fn settle(job: &Job) -> Result<(), EngineError> {
    let mut waited = 0;
    while waited < job.settle_ms {
        check_cancelled(job)?;
        let pause = (job.settle_ms - waited).min(200);
        thread::sleep(Duration::from_millis(pause));
        waited += pause;
    }
    Ok(())
}

/// Looks at the page before, during and after the walk through it: which names change on their
/// own, how far the page goes, and what it holds with the page back at the top.
fn survey(
    driver: &mut Driver,
    session: &SessionOpenResult,
    job: &Job,
    clock: &mut Clock,
    screens_dir: &Path,
    max_steps: u64,
    screenshot_warning: &mut Option<Warning>,
) -> Result<Survey, EngineError> {
    // What every element is called before the walk, to see which names change by the end.
    let before = timed(&mut clock.read, || read_frames(driver, session, job)).map(|reading| names_of(&reading))?;
    let watched_since = Instant::now();

    let walk = if job.crops {
        walk_with_pictures(driver, session, job, clock, screens_dir, max_steps, screenshot_warning)?
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
        return_to_top(driver, session, job, clock)?;
    }
    if let Some(left) = NAME_WATCH.checked_sub(watched_since.elapsed()) {
        check_cancelled(job)?;
        thread::sleep(left);
    }
    let mut reading = timed(&mut clock.read, || read_frames(driver, session, job))?;
    mark_changed_names(&mut reading, &before);
    let reached_end = reached_end.unwrap_or_else(|| steps == 0 && at_end(&reading.scroll));
    Ok(Survey {
        reading,
        screens,
        steps,
        reached_end,
        removals,
    })
}

fn return_to_top(
    driver: &mut Driver,
    session: &SessionOpenResult,
    job: &Job,
    clock: &mut Clock,
) -> Result<(), EngineError> {
    let wait_for = if job.crops {
        ScrollWait::Animations
    } else {
        ScrollWait::Height
    };
    timed(&mut clock.scroll, || {
        driver.until(job.cancel).scroll(&PageScrollParams {
            session_id: session.session_id.clone(),
            to: Some(ScrollTo::Top),
            by_y: None,
            wait_for: Some(wait_for),
        })
    })?;
    Ok(())
}

/// Cuts a crop for every component, saves the overview picture and removes the screens.
#[allow(clippy::too_many_arguments)]
fn save_pictures(
    driver: &mut Driver,
    session: &SessionOpenResult,
    job: &Job,
    progress: Progress,
    clock: &mut Clock,
    components: &mut [Component],
    screens: &[Screen],
    screenshot_warning: &mut Option<Warning>,
) -> Result<(), EngineError> {
    check_cancelled(job)?;
    let saved = timed(&mut clock.crop, || {
        save_component_crops(components, screens, job.viewport.width as f64, job.dir)
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
            screenshot(
                driver,
                job.cancel,
                &session.session_id,
                &page_png,
                true,
                screenshot_warning,
            )
        })?;
    if !whole_page {
        if let Some(first) = screens.first() {
            fs::copy(&first.image, &page_png)?;
        }
    }
    fs::remove_dir_all(job.dir.join("screens"))?;
    Ok(())
}

/// Writes every warning of the target to the event log.
fn announce_warnings(progress: Progress, job: &Job, warnings: &[Warning]) {
    for item in warnings {
        progress.emit(
            "warning",
            Some(job.name),
            item.message.clone(),
            json!({ "code": item.code }),
        );
    }
}
