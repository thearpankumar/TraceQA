//! Component extraction for one browser at one viewport.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};
use traceqa_protocol::types::{
    BrowserInfo, BrowserTarget, CollectorResult, Component, ComponentsFile, Event, FrameInfo, PageEvalParams,
    PageGotoParams, PageScreenshotParams, PageScrollParams, Rect, ScrollInfo, ScrollTo, SessionOpenParams,
    SessionOpenResult, Viewport, WaitUntil, Warning,
};
use traceqa_recorder::{now_rfc3339, save_crops, CropRequest, PageImage, RunWriter};

use crate::driver::{Driver, Drivers, DEFAULT_TIMEOUT_MS};
use crate::group::{group, Located};
use crate::run::RUN_FOLDER_VERSION;
use crate::{fingerprint, EngineError, COLLECTOR};

/// How many screens the engine scrolls through before it stops reading a very long page.
const DEFAULT_SCROLL_STEPS: u64 = 50;

/// Records events in the run folder and passes them to whoever is listening live.
pub(crate) struct Progress<'a> {
    pub writer: &'a mut RunWriter,
    pub listener: &'a mut dyn FnMut(&Event),
}

impl Progress<'_> {
    pub fn emit(&mut self, kind: &str, target: Option<&str>, message: String, data: Value) {
        match self.writer.event(kind, target, message, data) {
            Ok(event) => (self.listener)(&event),
            Err(error) => eprintln!("traceqa: could not write events.jsonl: {error}"),
        }
    }
}

/// Everything that identifies one extraction.
pub(crate) struct Job<'a> {
    pub url: &'a str,
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
    progress: &mut Progress,
) -> Result<(ComponentsFile, BrowserInfo), EngineError> {
    let driver = drivers.for_browser(&job.target.browser)?;
    let session = driver.open_session(&SessionOpenParams {
        target: job.target.clone(),
        viewport: job.viewport.clone(),
        locale: None,
    })?;
    let result = read_page(driver, &session, job, progress);
    // A driver that stopped answering cannot close anything. It is replaced before its next use.
    if !driver.is_lost() {
        if let Err(error) = driver.close_session(&session.session_id) {
            eprintln!("traceqa: could not close the {} session: {error}", job.target.browser);
        }
    }
    result.map(|file| (file, session.browser))
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
                    format!("Could not read the frame at {}: {reason}", frame.url),
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

fn read_page(
    driver: &mut Driver,
    session: &SessionOpenResult,
    job: &Job,
    progress: &mut Progress,
) -> Result<ComponentsFile, EngineError> {
    let session_id = &session.session_id;
    let page = driver.goto(&PageGotoParams {
        session_id: session_id.clone(),
        url: job.url.to_string(),
        wait_until: Some(WaitUntil::Settled),
        timeout_ms: Some(job.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS)),
    })?;
    progress.emit(
        "page.opened",
        Some(job.name),
        format!(
            "Opened {} in {} {}",
            page.final_url, session.browser.name, session.browser.version
        ),
        json!({ "finalUrl": page.final_url, "status": page.status, "title": page.title }),
    );

    // A timed intro or splash screen gives no sign of itself, so the caller says how long to wait.
    let mut waited = 0;
    while waited < job.settle_ms {
        check_cancelled(job)?;
        let pause = (job.settle_ms - waited).min(200);
        thread::sleep(Duration::from_millis(pause));
        waited += pause;
    }

    // Go through the page one screen at a time, the way a person scrolls. That loads lazy
    // content, plays reveal animations, and gives a picture of every part as it really looked.
    let screens_dir = job.dir.join("screens");
    if job.crops {
        fs::create_dir_all(&screens_dir)?;
    }
    let max_steps = job.max_scroll_steps.unwrap_or(DEFAULT_SCROLL_STEPS);
    let mut screens: Vec<Screen> = Vec::new();
    let mut screenshot_warning = None;
    let mut steps = 0;
    let (mut reading, reached_end) = loop {
        check_cancelled(job)?;
        let reading = read_frames(driver, session, job)?;
        let scroll = &reading.scroll;
        let image = screens_dir.join(format!("{steps}.png"));
        if job.crops && screenshot(driver, session_id, &image, false, &mut screenshot_warning)? {
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

        let at_end = scroll.y + scroll.viewport_height >= scroll.height - 1.0;
        if at_end || steps >= max_steps {
            break (reading, at_end);
        }
        // A little less than a full screen, so nothing falls into the gap between two screens.
        let before = scroll.y;
        let scrolled = driver.scroll(&PageScrollParams {
            session_id: session_id.clone(),
            to: None,
            by_y: Some(scroll.viewport_height * 0.9),
        })?;
        steps += 1;
        if scrolled.y <= before {
            // The page did not move: it scrolls inside a container, or not at all.
            break (reading, true);
        }
    };
    if steps > 0 {
        // Read the final list with the page back at the top, so positions do not depend on where scrolling stopped.
        driver.scroll(&PageScrollParams {
            session_id: session_id.clone(),
            to: Some(ScrollTo::Top),
            by_y: None,
        })?;
        reading = read_frames(driver, session, job)?;
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
    progress.emit(
        "elements.collected",
        Some(job.name),
        format!("Found {} elements in {frames} frames", elements.len()),
        json!({ "elements": elements.len(), "frames": frames }),
    );

    let mut components = group(elements);
    let instance_count: usize = components.iter().map(|component| component.instances.len()).sum();
    progress.emit(
        "components.grouped",
        Some(job.name),
        format!("Grouped {instance_count} elements into {} components", components.len()),
        json!({ "components": components.len(), "instances": instance_count }),
    );

    if job.crops {
        check_cancelled(job)?;
        let saved = save_component_crops(&mut components, &screens, job.viewport.width as f64, job.dir)?;
        progress.emit(
            "crop.saved",
            Some(job.name),
            format!("Saved {saved} crops"),
            json!({ "crops": saved }),
        );

        // page.png is an overview: the whole page when the browser can capture that, else the first screen.
        let page_png = job.dir.join("page.png");
        let whole_page = session.capabilities.full_page_screenshot
            && screenshot(driver, session_id, &page_png, true, &mut screenshot_warning)?;
        if !whole_page {
            if let Some(first) = screens.first() {
                fs::copy(&first.image, &page_png)?;
            }
        }
        fs::remove_dir_all(&screens_dir)?;
    }

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
        run_id: progress.writer.run_id().to_string(),
        url: job.url.to_string(),
        final_url: page.final_url,
        title,
        browser: session.browser.clone(),
        viewport: job.viewport.clone(),
        captured_at: now_rfc3339(),
        components,
        warnings,
    })
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
